import { createInterface } from "node:readline";
import { access, mkdir } from "node:fs/promises";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const sessionId = process.env.JKC_BROWSER_SESSION_ID || "browser";

// —— 运行态 ——
// 无头优先：本 sidecar 只做无头执行（页面经 screencast 帧流回放给前端），
// 不再提供有头窗口 / 最小化 / 恢复等 OS 窗口操作。
let context = null;
let page = null;
let cdp = null;
let currentUrl = null;
let downloadsDir = null;
/** ref -> { frameIndex, role, name, nth, backendNodeId? }（backendNodeId 仅主 frame） */
let elementRefs = new Map();
let nextElementRefId = 1;
let currentViewport = null;
let storedOptions = null;
let refsInvalidatedByNavigation = false;
let lastSnapshotRefCount = 0;
/** 快照时刻的 frame 数组：ref 的 frameIndex 指向这里，点击时校验仍存活。 */
let snapshotFrames = [];
/** 快照时刻各 frame 的 (role, name) → 出现计数：局部快照的 nth 必须续用
 * 全 frame 计数（getByRole().nth 数的是整帧匹配序号），否则同名元素点错。 */
let snapshotOccurrences = new Map();
/** 拟人化（反爬）开关：默认关闭，Agent 工具传 humanize:true 时一次性开启。 */
let humanizeEnabled = false;

function write(value) {
  process.stdout.write(`${JSON.stringify(value)}\n`);
}

function status(state, message = null) {
  write({
    event: "status",
    sessionId,
    status: { sessionId, state, url: currentUrl, message },
  });
}

function log(message) {
  write({ event: "log", sessionId, message });
}

function respond(id, result) {
  write({ id, ok: true, result });
}

function reject(id, error) {
  write({ id, ok: false, error: error instanceof Error ? error.message : String(error) });
}

function rejectStructured(id, errorType, message, extra = null) {
  const response = { id, ok: false, error: message, errorType };
  if (extra) Object.assign(response, extra);
  write(response);
}

function cloakCandidateDirs() {
  return [
    process.env.JKC_BROWSER_NODE_MODULES,
    join(process.cwd(), "node_modules"),
    join(new URL(".", import.meta.url).pathname, "node_modules"),
  ].filter(Boolean);
}

async function fileExists(path) {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
}

/** 解析 cloakbrowser 包目录（引擎入口与拟人层子模块共用）。 */
async function findCloakBrowserDir() {
  for (const nodeModulesDir of cloakCandidateDirs()) {
    const candidate = join(nodeModulesDir, "cloakbrowser");
    if (await fileExists(join(candidate, "dist", "index.js"))) return candidate;
  }
  return null;
}

async function importCloakBrowser() {
  const dir = await findCloakBrowserDir();
  if (dir) return import(pathToFileURL(join(dir, "dist", "index.js")).href);
  try {
    return await import("cloakbrowser");
  } catch (error) {
    const searched = cloakCandidateDirs().map((path) => `- ${path}`).join("\n");
    throw new Error(
      `无法加载 cloakbrowser ESM 入口：${error instanceof Error ? error.message : String(error)}\n已搜索：\n${searched}`,
    );
  }
}

async function importCloakHuman() {
  const dir = await findCloakBrowserDir();
  if (dir) return import(pathToFileURL(join(dir, "dist", "human", "index.js")).href);
  return import("cloakbrowser/human");
}

/**
 * 按需启用拟人化（反爬）：单向开关——一旦页面出现人机验证/反爬拦截，
 * Agent 在工具入参传 humanize:true 后本会话持续拟人（含后续新开页面）。
 * 频谱见基准：拟人化让点击 ~1s、逐字符输入 ~170ms/字，仅反爬场景值得。
 */
async function ensureHumanize() {
  if (humanizeEnabled || !context) return;
  status("busy", "正在启用反爬拟人化");
  const human = await importCloakHuman();
  human.patchContext(context, human.resolveConfig("default"));
  humanizeEnabled = true;
  log("已启用 cloakbrowser 拟人化（反爬模式）");
}

async function startScreencast(targetPage, viewport) {
  if (cdp) {
    await cdp.detach().catch(() => undefined);
    cdp = null;
  }
  cdp = await targetPage.context().newCDPSession(targetPage);
  cdp.on("Page.screencastFrame", async (frame) => {
    write({
      event: "frame",
      sessionId,
      data: `data:image/jpeg;base64,${frame.data}`,
      width: viewport?.width || 1280,
      height: viewport?.height || 800,
    });
    await cdp.send("Page.screencastFrameAck", { sessionId: frame.sessionId }).catch(() => {});
  });
  await cdp.send("Page.startScreencast", {
    format: "jpeg",
    quality: 72,
    everyNthFrame: 1,
  });
}

async function ensureStarted(params) {
  if (context && page) return;

  status("starting", "正在加载 CloakBrowser");
  const cloak = await importCloakBrowser();
  if (typeof cloak.binaryInfo === "function") {
    const info = cloak.binaryInfo();
    if (!info.installed) {
      status("downloading", "正在下载 CloakBrowser patched Chromium");
      await cloak.ensureBinary();
    }
  }

  const viewport = params.viewport || { width: 1280, height: 800 };
  currentViewport = viewport;
  storedOptions = { ...params };
  await launchContext(params, viewport);
}

async function launchContext(params, viewport) {
  const cloak = await importCloakBrowser();
  if (typeof cloak.binaryInfo === "function") {
    const info = cloak.binaryInfo();
    if (!info.installed) {
      status("downloading", "正在下载 CloakBrowser patched Chromium");
      await cloak.ensureBinary();
    }
  }

  downloadsDir = join(params.userDataDir, "downloads");
  await mkdir(downloadsDir, { recursive: true });
  const options = {
    userDataDir: params.userDataDir,
    // 无头优先：执行细节不弹 OS 窗口，前端经 screencast 帧流回放。
    headless: true,
    // 拟人化默认关闭（性能优先），由 Agent 工具入参 humanize 按需开启。
    humanize: false,
    viewport,
    acceptDownloads: true,
    downloadsPath: downloadsDir,
  };
  if (params.profileDirectory) {
    options.args = [`--profile-directory=${params.profileDirectory}`];
  }
  if (params.proxy) options.proxy = params.proxy;
  if (params.locale) options.locale = params.locale;
  if (params.timezone) options.timezone = params.timezone;

  status("launching", "正在启动无头 CloakBrowser");
  context = await cloak.launchPersistentContext(options);
  page = context.pages()[0] || (await context.newPage());
  if (humanizeEnabled) {
    // 会话内已开启过拟人化：重启后的新上下文需要重新打补丁。
    await ensureHumanize();
  }
  attachPageListeners(page);
  currentUrl = page.url();
  await startScreencast(page, viewport);
  status("ready", null);
}

function attachPageListeners(targetPage) {
  targetPage.on("framenavigated", (frame) => {
    if (frame === targetPage.mainFrame()) {
      currentUrl = targetPage.url();
      const hadRefs = elementRefs.size > 0;
      clearElementRefs();
      if (hadRefs) refsInvalidatedByNavigation = true;
      status("ready", null);
    }
  });
  // 下载全程事件驱动：捕获即保存（失败进日志），工具调用只做短探测回传元信息。
  targetPage.on("download", (download) => {
    const suggested = download.suggestedFilename();
    status("downloading", `正在下载：${suggested}`);
    log(`检测到下载：${suggested}`);
    saveDownload(download).catch((error) => {
      log(`下载保存失败：${error instanceof Error ? error.message : String(error)}`);
    });
  });
  targetPage.on("close", () => {
    status("closed", "浏览器页面已关闭");
  });
}

function cleanupCdp() {
  if (cdp) {
    cdp.send("Page.stopScreencast").catch(() => undefined);
    cdp.detach().catch(() => undefined);
    cdp = null;
  }
}

function timeout(params) {
  return Math.max(1, Number(params.timeout || 60000));
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function isLikelyDownloadAbort(error) {
  const message = error instanceof Error ? error.message : String(error);
  return message.includes("net::ERR_ABORTED") || message.includes("Download is starting");
}

function clearElementRefs() {
  elementRefs = new Map();
  snapshotFrames = [];
  snapshotOccurrences = new Map();
  // ref 序号不重置：跨快照全局唯一，旧 ref 永不与新 ref 碰撞。
}

/** 取某 frame 的出现计数（无则建并登记）——全量/局部快照共用一份计数。 */
function occurrenceFor(frameIndex) {
  let map = snapshotOccurrences.get(frameIndex);
  if (!map) {
    map = new Map();
    snapshotOccurrences.set(frameIndex, map);
  }
  return map;
}

function normalizeRef(ref) {
  const normalized = String(ref || "").trim();
  if (!normalized) throw new Error("缺少元素 ref；请先调用 browser_read_text 获取页面快照");
  return normalized;
}

function refTarget(ref) {
  const normalized = normalizeRef(ref);
  const target = elementRefs.get(normalized);
  if (!target) {
    const err = new Error(`未知或已失效的元素 ref：${normalized}。请重新调用 browser_read_text 获取最新快照`);
    err.errorType = "ref_expired";
    err.expiredRef = normalized;
    throw err;
  }
  return target;
}

/** 解析 ref 的 frame；快照后 frame 已销毁/替换时返回 null。 */
function resolveFrame(frameIndex) {
  const frame = snapshotFrames[frameIndex];
  if (!frame || !page || !page.frames().includes(frame)) return null;
  return frame;
}

// —— CDP AX 角色（camelCase）→ Playwright ARIA role 映射 ——
// 语义定位用；映射不到的角色退回坐标点击（主 frame）或报错（iframe 内）。
const AX_ROLE_TO_ARIA = {
  button: "button",
  pushButton: "button",
  toggleButton: "button",
  link: "link",
  heading: "heading",
  textBox: "textbox",
  searchBox: "searchbox",
  spinButton: "spinbutton",
  slider: "slider",
  switch: "switch",
  checkBox: "checkbox",
  radioButton: "radio",
  comboBox: "combobox",
  listBox: "listbox",
  listItem: "listitem",
  list: "list",
  menu: "menu",
  menuItem: "menuitem",
  menuBar: "menubar",
  menuListPopup: "menu",
  tab: "tab",
  tabList: "tablist",
  table: "table",
  row: "row",
  rowHeader: "row",
  columnHeader: "columnheader",
  grid: "grid",
  tree: "tree",
  treeItem: "treeitem",
  image: "img",
  figure: "figure",
  dialog: "dialog",
  alertDialog: "alertdialog",
  alert: "alert",
  form: "form",
  article: "article",
  main: "main",
  navigation: "navigation",
  complementary: "complementary",
  contentInfo: "contentinfo",
  banner: "banner",
  region: "region",
  group: "group",
  note: "note",
  option: "option",
  progressIndicator: "progressbar",
  definition: "definition",
  paragraph: "paragraph",
  time: "time",
  meter: "meter",
  math: "math",
  log: "log",
  marquee: "marquee",
  status: "status",
  toolbar: "toolbar",
  tooltip: "tooltip",
  feed: "feed",
  directory: "directory",
  document: "document",
  application: "application",
  cell: "cell",
  rowGroup: "rowgroup",
  genericRoot: "generic",
};

function toAriaRole(role) {
  if (!role) return null;
  if (AX_ROLE_TO_ARIA[role]) return AX_ROLE_TO_ARIA[role];
  // kebab-case（iframe ariaSnapshot 子树）已是 ARIA role，原样可用。
  if (/^[a-z][a-z-]*$/.test(role)) return role;
  // 兜底：首字母小写化（camelCase → 小写开头）后再查。
  const lowered = role.charAt(0).toLowerCase() + role.slice(1);
  return AX_ROLE_TO_ARIA[lowered] ?? null;
}

/**
 * ref → Playwright 语义定位器（getByRole / getByText）。
 * nth 是快照时该 (frame, role, name) 组合的出现序号，保证同名词表元素可区分。
 */
function semanticLocator(target) {
  const frame = resolveFrame(target.frameIndex);
  if (!frame) return null;
  const name = target.name || "";
  if (target.role === "text") {
    if (!name) return null;
    let locator = frame.getByText(name, { exact: true });
    if (target.nth) locator = locator.nth(target.nth);
    return locator;
  }
  const ariaRole = toAriaRole(target.role);
  if (!ariaRole) return null;
  let locator = name
    ? frame.getByRole(ariaRole, { name, exact: true })
    : frame.getByRole(ariaRole);
  if (target.nth) locator = locator.nth(target.nth);
  return locator;
}

function describeTarget(target) {
  const role = target.role || "node";
  return target.name ? `${role} "${target.name}"` : role;
}

// —— 快照渲染 ——

function axValue(value) {
  if (!value || value.value === undefined || value.value === null) return "";
  return String(value.value).trim();
}

function isUsefulAXNode(node) {
  if (!node || node.ignored) return false;
  const role = axValue(node.role);
  const name = axValue(node.name);
  const value = axValue(node.value);
  const description = axValue(node.description);
  return Boolean(role || name || value || description);
}

/** 推进 (role, name) 出现计数并返回序号——nth 的正确性依赖全树推进。 */
function countOccurrence(occurrence, role, name) {
  const key = `${role}\x00${name}`;
  const nth = occurrence.get(key) ?? 0;
  occurrence.set(key, nth + 1);
  return nth;
}

function axProperty(node, name) {
  const property = node.properties?.find((item) => item.name === name);
  return axValue(property?.value);
}

function formatAXNode(node, ref) {
  const role = axValue(node.role) || "node";
  const name = axValue(node.name);
  const value = axValue(node.value);
  const description = axValue(node.description);
  const states = [
    ["checked", axProperty(node, "checked")],
    ["pressed", axProperty(node, "pressed")],
    ["expanded", axProperty(node, "expanded")],
    ["selected", axProperty(node, "selected")],
    ["disabled", axProperty(node, "disabled")],
    ["focused", axProperty(node, "focused")],
    ["level", axProperty(node, "level")],
  ]
    .filter(([, value]) => value)
    .map(([key, value]) => `${key}=${value}`);

  const chunks = [role];
  if (ref) chunks.push(`[ref=${ref}]`);
  if (name) chunks.push(`"${name}"`);
  if (value) chunks.push(`value="${value}"`);
  if (description) chunks.push(`description="${description}"`);
  if (states.length) chunks.push(`[${states.join(", ")}]`);
  return chunks.join(" ");
}

/**
 * 主 frame CDP AX 树 → 带 ref 的缩进文本。
 * 字符上限用累计长度判断（O(n)），不再在循环里反复 join 全量行。
 */
function formatAXTree(nodes, params, occurrence) {
  const maxNodes = Math.max(1, Number(params.maxNodes || 600));
  const maxChars = Math.max(1_000, Number(params.maxChars || 80_000));
  const nodeById = new Map(nodes.map((node) => [node.nodeId, node]));
  const referenced = new Set();
  for (const node of nodes) {
    for (const childId of node.childIds || []) referenced.add(childId);
  }
  const roots = nodes.filter((node) => !referenced.has(node.nodeId));
  const startNodes = roots.length ? roots : nodes.slice(0, 1);
  const lines = [];
  const visited = new Set();
  let emitted = 0;
  let charLen = 0;
  let truncated = false;

  function walk(node, depth) {
    if (!node || visited.has(node.nodeId)) return;
    visited.add(node.nodeId);
    if (isUsefulAXNode(node)) {
      const role = axValue(node.role);
      const name = axValue(node.name);
      const nth = countOccurrence(occurrence, role, name);
      if (!truncated && emitted < maxNodes) {
        let ref = "";
        if (node.backendDOMNodeId) {
          ref = `r${nextElementRefId++}`;
          elementRefs.set(ref, { frameIndex: 0, role, name, nth, backendNodeId: node.backendDOMNodeId });
        }
        const line = `${"  ".repeat(depth)}- ${formatAXNode(node, ref)}`;
        if (charLen + line.length > maxChars) {
          truncated = true;
        } else {
          lines.push(line);
          emitted += 1;
          charLen += line.length + 1;
        }
      }
    }
    // 截断后仍继续遍历：nth 计数必须覆盖全树，否则截断尾部的
    // 同名元素序号会与 getByRole 的全量序号错位。
    for (const childId of node.childIds || []) {
      walk(nodeById.get(childId), depth + 1);
    }
  }

  for (const root of startNodes) {
    walk(root, 0);
  }

  let text = lines.join("\n").trim();
  if (!text) text = "(Accessibility Tree 为空)";
  if (truncated && text.length >= maxChars) {
    text = `${text.slice(0, maxChars)}\n...`;
  }
  return { text, emitted, truncated };
}

/**
 * iframe 子树：locator.ariaSnapshot() 的 YAML → 带 ref 的缩进文本。
 * YAML 行形如 `  - button "登录"`（后缀可带 `: value` / ` [checked]`），
 * `/xxx:` 属性行原样保留不建 ref。
 */
function parseAriaSnapshot(text, frameIndex, occurrence, baseDepth) {
  const lines = text.split("\n");
  const out = [];
  let emitted = 0;
  for (const rawLine of lines) {
    if (!rawLine.trim()) continue;
    const match = /^(\s*)-\s+(.+)$/.exec(rawLine);
    if (!match) {
      out.push(`${"  ".repeat(baseDepth)}${rawLine}`);
      continue;
    }
    const depth = baseDepth + Math.floor(match[1].length / 2);
    const rest = match[2];
    if (rest.startsWith("/")) {
      out.push(`${"  ".repeat(depth)}- ${rest}`);
      continue;
    }
    const roleMatch = /^([A-Za-z][A-Za-z0-9-]*)/.exec(rest);
    const role = roleMatch ? roleMatch[1] : null;
    let name = "";
    let tail = "";
    if (role) {
      let cursor = role.length;
      while (cursor < rest.length && rest[cursor] === " ") cursor += 1;
      if (rest[cursor] === '"') {
        let value = "";
        let index = cursor + 1;
        while (index < rest.length) {
          const ch = rest[index];
          if (ch === "\\" && index + 1 < rest.length) {
            value += rest[index + 1];
            index += 2;
            continue;
          }
          if (ch === '"') break;
          value += ch;
          index += 1;
        }
        name = value;
        tail = rest.slice(Math.min(index + 1, rest.length));
      } else {
        tail = rest.slice(cursor);
      }
    }
    const line = (() => {
      if (!role) return `${"  ".repeat(depth)}- ${rest}`;
      // `- text: 内容` 形态：名称在尾巴里，提取出来供 getByText 语义定位。
      if (role === "text" && !name && tail.startsWith(":")) {
        name = tail.slice(1).trim().replace(/^"|"$/g, "");
        tail = "";
      }
      const nth = countOccurrence(occurrence, role, name);
      const ref = `r${nextElementRefId++}`;
      elementRefs.set(ref, { frameIndex, role, name, nth });
      const chunks = [role, `[ref=${ref}]`];
      if (name) chunks.push(`"${name}"`);
      const tailText = tail.trim();
      if (tailText) chunks.push(tailText);
      return `${"  ".repeat(depth)}- ${chunks.join(" ")}`;
    })();
    out.push(line);
    emitted += 1;
  }
  return { text: out.join("\n"), emitted };
}

function shortFrameLabel(frame) {
  const url = frame.url();
  if (!url || url === "about:blank") return frame.name() || "iframe";
  try {
    const parsed = new URL(url);
    return parsed.hostname || url.slice(0, 40);
  } catch {
    return url.slice(0, 40);
  }
}

/**
 * 全量快照：主 frame CDP AX 树 + 各子 frame 的 ariaSnapshot 子树。
 * 子 frame 经语义定位可达（同源/跨源一致），弥补 CDP AX 树看不到 iframe 内容的盲区。
 */
async function readAccessibilitySnapshot(params = {}) {
  status("busy", "正在读取页面可访问性树");
  await cdp.send("Accessibility.enable");
  try {
    clearElementRefs();
    refsInvalidatedByNavigation = false;
    snapshotFrames = page.frames();
    const occurrence = occurrenceFor(0);

    const result = await cdp.send("Accessibility.getFullAXTree");
    const main = formatAXTree(result.nodes || [], params, occurrence);
    const sections = [main.text];

    for (let index = 1; index < snapshotFrames.length; index += 1) {
      const frame = snapshotFrames[index];
      let frameText = null;
      try {
        const body = frame.locator("body");
        frameText = await body.ariaSnapshot({ timeout: 2000 });
      } catch {
        // 无 body / 超时 / 已卸载的 frame：跳过，不让子树失败拖垮整次快照。
        continue;
      }
      if (!frameText.trim()) continue;
      const parsed = parseAriaSnapshot(frameText, index, occurrenceFor(index), 1);
      sections.push(`- iframe [frame=${index}] "${shortFrameLabel(frame)}"\n${parsed.text}`);
    }

    let text = sections.join("\n");
    const maxChars = Math.max(1_000, Number(params.maxChars || 80_000));
    let truncated = main.truncated;
    if (text.length > maxChars) {
      text = `${text.slice(0, maxChars)}\n...`;
      truncated = true;
    }
    currentUrl = page.url();
    lastSnapshotRefCount = elementRefs.size;
    status("ready", null);
    return {
      text: `# Accessibility Tree Snapshot\nurl: ${currentUrl || ""}\nnode_count: ${(result.nodes || []).length}\ntruncated: ${truncated}\n\n${text}`,
      nodeCount: (result.nodes || []).length,
      emittedNodeCount: main.emitted,
      refCount: elementRefs.size,
      truncated,
      url: currentUrl,
      source: "chrome_devtools_protocol_accessibility_plus_frame_aria_snapshot",
    };
  } finally {
    await cdp.send("Accessibility.disable").catch(() => undefined);
  }
}

/** 局部快照（按 ref）：主 frame 走 CDP 局部树，iframe 元素走 locator.ariaSnapshot()。 */
async function readPartialSnapshot(ref, params) {
  const target = refTarget(ref);
  status("busy", `正在读取元素子树：${describeTarget(target)}`);
  if (target.frameIndex === 0 && target.backendNodeId) {
    await cdp.send("Accessibility.enable");
    try {
      const result = await cdp.send("Accessibility.getPartialAXTree", {
        backendNodeId: target.backendNodeId,
        fetchRelatives: false,
      });
      // 续用全 frame 计数：局部树内第 2 个同名元素在全帧语境下未必是第 2 个。
      const snapshot = formatAXTree(result.nodes || [], params, occurrenceFor(0));
      currentUrl = page.url();
      lastSnapshotRefCount = elementRefs.size;
      status("ready", null);
      return {
        text: `# Accessibility Tree Snapshot\nurl: ${currentUrl || ""}\nnode_count: ${(result.nodes || []).length}\ntruncated: ${snapshot.truncated}\n\n${snapshot.text}`,
        nodeCount: (result.nodes || []).length,
        emittedNodeCount: snapshot.emitted,
        refCount: elementRefs.size,
        truncated: snapshot.truncated,
        url: currentUrl,
        source: "chrome_devtools_protocol_partial_ax_tree",
      };
    } finally {
      await cdp.send("Accessibility.disable").catch(() => undefined);
    }
  }

  const locator = semanticLocator(target);
  if (!locator) {
    throw new Error(`ref=${normalizeRef(ref)} 位于已不可达的 frame，请重新调用 browser_read_text 获取最新快照`);
  }
  const frameText = await locator.ariaSnapshot({ timeout: 5000 }).catch(() => null);
  const rootLine = `- ${describeTarget(target)} [ref=${normalizeRef(ref)}]`;
  const body = frameText && frameText.trim()
    ? parseAriaSnapshot(frameText, target.frameIndex, occurrenceFor(target.frameIndex), 1).text
    : "（该元素无子树）";
  currentUrl = page.url();
  lastSnapshotRefCount = elementRefs.size;
  status("ready", null);
  return {
    text: `# Accessibility Tree Snapshot（ref=${normalizeRef(ref)} 局部树）\nurl: ${currentUrl || ""}\nnode_count: 1\ntruncated: false\n\n${rootLine}\n${body}`,
    nodeCount: 1,
    emittedNodeCount: 1,
    refCount: elementRefs.size,
    truncated: false,
    url: currentUrl,
    source: "playwright_aria_snapshot",
  };
}

// —— 坐标兜底（仅主 frame）——

async function elementCenterByRef(ref) {
  const target = refTarget(ref);
  if (!target.backendNodeId) {
    throw new Error(`ref=${normalizeRef(ref)} 缺少主文档节点句柄，无法坐标定位`);
  }
  await cdp.send("DOM.scrollIntoViewIfNeeded", { backendNodeId: target.backendNodeId });
  const box = await cdp.send("DOM.getBoxModel", { backendNodeId: target.backendNodeId });
  const quad = box.model?.content?.length ? box.model.content : box.model?.border;
  if (!quad || quad.length < 8) {
    throw new Error(`无法计算元素 ref=${normalizeRef(ref)} 的操作区域`);
  }
  const xs = [quad[0], quad[2], quad[4], quad[6]];
  const ys = [quad[1], quad[3], quad[5], quad[7]];
  return {
    x: xs.reduce((sum, value) => sum + value, 0) / xs.length,
    y: ys.reduce((sum, value) => sum + value, 0) / ys.length,
  };
}

async function saveDownload(download) {
  const suggestedFilename = download.suggestedFilename();
  status("downloading", `正在保存下载：${suggestedFilename}`);
  const targetPath = join(downloadsDir, suggestedFilename);
  await download.saveAs(targetPath);
  const failure = await download.failure();
  if (failure) throw new Error(`下载失败：${failure}`);
  status("ready", `下载完成：${suggestedFilename}`);
  log(`下载完成：${targetPath}`);
  return {
    downloaded: true,
    suggestedFilename,
    path: targetPath,
    url: download.url(),
  };
}

/**
 * 动作 + 下载短探测：下载本身由全局监听器异步保存，这里只在动作后做
 * 120ms 探测回传「是否触发下载 + 文件名」元信息，不阻塞等待下载完成。
 */
async function withDownloadProbe(actionLabel, action) {
  status("busy", actionLabel);
  const downloadPromise = page
    .waitForEvent("download", { timeout: 30_000 })
    .then((download) => ({ filename: download.suggestedFilename() }))
    .catch(() => null);
  try {
    const actionResult = await action();
    const downloadMeta = await Promise.race([downloadPromise, delay(120).then(() => null)]);
    currentUrl = page.url();
    status("ready", null);
    return downloadMeta
      ? { ok: true, ...actionResult, download: downloadMeta.filename, url: currentUrl }
      : { ok: true, ...actionResult, url: currentUrl };
  } catch (error) {
    if (!isLikelyDownloadAbort(error)) {
      if (page) {
        currentUrl = page.url();
        status("ready", null);
      }
      throw error;
    }
    const downloadMeta = await Promise.race([downloadPromise, delay(120).then(() => null)]);
    currentUrl = page.url();
    status("ready", null);
    return downloadMeta
      ? { ok: true, download: downloadMeta.filename, url: currentUrl }
      : { ok: true, url: currentUrl };
  }
}

/** 语义定位优先的元素点击；主 frame 语义失败时回退坐标点击。 */
async function clickByTarget(target, params) {
  const startedAt = Date.now();
  const locator = semanticLocator(target);
  if (locator) {
    try {
      await locator.click({ timeout: timeout(params) });
      return { mode: "semantic", ms: Date.now() - startedAt };
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (target.frameIndex !== 0 || !target.backendNodeId) {
        throw new Error(
          `语义定位点击失败（${describeTarget(target)}）：${message}。页面可能已变化，请重新调用 browser_read_text 获取最新快照后重试`,
        );
      }
      log(`语义定位失败，回退坐标点击：${message.split("\n")[0]}`);
    }
  }
  const point = await elementCenterByRef(normalizeRef(params.ref));
  await page.mouse.click(point.x, point.y);
  return { mode: "coordinate", x: point.x, y: point.y, ms: Date.now() - startedAt };
}

async function run(method, params = {}) {
  if (method === "start") {
    await ensureStarted(params);
    return { status: { sessionId, state: "ready", url: currentUrl, message: null } };
  }
  if (method === "close") {
    await cdp?.send("Page.stopScreencast").catch(() => undefined);
    await cdp?.detach().catch(() => undefined);
    cdp = null;
    await context?.close();
    context = null;
    page = null;
    status("closed", "浏览器已关闭");
    return { status: { sessionId, state: "closed", url: currentUrl, message: null } };
  }

  if (!page) {
    throw new Error("CloakBrowser 尚未启动，请先调用 browser_open_url 或 browser_start");
  }

  switch (method) {
    case "open_url": {
      if (!params.url) throw new Error("缺少必填参数 url");
      const result = await withDownloadProbe(`正在打开：${params.url}`, async () => {
        if (params.humanize) await ensureHumanize();
        await page.goto(params.url, { waitUntil: "domcontentloaded", timeout: timeout(params) });
        return { title: await page.title() };
      });
      return result.download ? result : { url: currentUrl, title: result.title };
    }
    case "back": {
      const result = await withDownloadProbe("正在返回上一页", async () => {
        await page.goBack({ waitUntil: "domcontentloaded", timeout: timeout(params) });
        return { title: await page.title() };
      });
      currentUrl = page.url();
      return { ok: true, url: currentUrl, title: result.title };
    }
    case "reload": {
      return withDownloadProbe("正在刷新页面", async () => {
        await page.reload({ waitUntil: "domcontentloaded", timeout: timeout(params) });
        return { title: await page.title() };
      });
    }
    case "click": {
      const target = params.ref ? refTarget(params.ref) : null;
      const label = target
        ? `正在点击：${describeTarget(target)}`
        : "正在点击页面坐标";
      return withDownloadProbe(label, async () => {
        if (params.humanize) await ensureHumanize();
        if (params.ref) {
          const outcome = await clickByTarget(target, params);
          return { ref: normalizeRef(params.ref), ...outcome };
        }
        if (Number.isFinite(params.x) && Number.isFinite(params.y)) {
          await page.mouse.click(Number(params.x), Number(params.y));
          return {};
        }
        throw new Error("browser_click 需要 ref；请先调用 browser_read_text 获取元素 ref");
      });
    }
    case "type": {
      if (!params.ref) throw new Error("缺少必填参数 ref；请先调用 browser_read_text 获取输入元素 ref");
      if (typeof params.text !== "string") throw new Error("缺少必填参数 text");
      const target = refTarget(params.ref);
      status("busy", `正在输入：${describeTarget(target)}`);
      if (params.humanize) await ensureHumanize();
      const locator = semanticLocator(target);
      const preview = params.text.length > 24 ? `${params.text.slice(0, 24)}…` : params.text;
      if (locator) {
        try {
          // 真实按键序列（pressSequentially）：拟人化关闭时零延迟直发，
          // 开启后节奏由 cloakbrowser 拟人层接管。
          await locator.pressSequentially(params.text, { delay: 0, timeout: timeout(params) });
          currentUrl = page.url();
          status("ready", null);
          return { ok: true, ref: normalizeRef(params.ref), mode: "semantic", text: preview };
        } catch (error) {
          const message = error instanceof Error ? error.message : String(error);
          if (target.frameIndex !== 0 || !target.backendNodeId) {
            throw new Error(
              `语义定位输入失败（${describeTarget(target)}）：${message}。页面可能已变化，请重新调用 browser_read_text 获取最新快照后重试`,
            );
          }
          log(`语义定位输入失败，回退坐标点击 + 键盘输入：${message.split("\n")[0]}`);
        }
      }
      const point = await elementCenterByRef(params.ref);
      await page.mouse.click(point.x, point.y);
      await page.keyboard.type(params.text, { delay: 0 });
      currentUrl = page.url();
      status("ready", null);
      return { ok: true, ref: normalizeRef(params.ref), mode: "coordinate", text: preview };
    }
    case "press": {
      if (!params.key) throw new Error("缺少必填参数 key");
      return withDownloadProbe(`正在发送按键：${params.key}`, async () => {
        if (params.humanize) await ensureHumanize();
        await page.keyboard.press(params.key);
        return {};
      });
    }
    case "wait_for": {
      const state = params.loadState || "domcontentloaded";
      await page.waitForLoadState(state, { timeout: timeout(params) });
      return { ok: true, loadState: state };
    }
    case "read_text": {
      if (params.ref) return readPartialSnapshot(params.ref, params);
      return readAccessibilitySnapshot(params);
    }
    case "screenshot": {
      const bytes = await page.screenshot({ type: "png", fullPage: Boolean(params.fullPage) });
      return { data: `data:image/png;base64,${bytes.toString("base64")}` };
    }
    default:
      throw new Error(`未知浏览器方法：${method}`);
  }
}

const rl = createInterface({ input: process.stdin, crlfDelay: Infinity });
status("booting", "CloakBrowser sidecar 已启动（无头模式）");

rl.on("line", async (line) => {
  let request;
  try {
    request = JSON.parse(line);
  } catch (error) {
    log(`无法解析 JSON-RPC 请求：${error.message}`);
    return;
  }
  try {
    respond(request.id, await run(request.method, request.params || {}));
  } catch (error) {
    if (error.errorType) {
      rejectStructured(request.id, error.errorType, error.message, {
        expiredRef: error.expiredRef || null,
        refsInvalidatedByNavigation,
        currentUrl,
        lastSnapshotRefCount,
      });
    } else {
      reject(request.id, error);
    }
  }
});

process.on("SIGTERM", async () => {
  await context?.close().catch(() => undefined);
  process.exit(0);
});

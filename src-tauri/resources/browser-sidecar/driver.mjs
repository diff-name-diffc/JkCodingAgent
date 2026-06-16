import { createInterface } from "node:readline";
import { access, mkdir } from "node:fs/promises";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const sessionId = process.env.JKC_BROWSER_SESSION_ID || "browser";

let context = null;
let page = null;
let cdp = null;
let currentUrl = null;
let downloadsDir = null;
let elementRefs = new Map();
let nextElementRefId = 1;
let savedBounds = null;
let headlessMode = false;
let isHeadedWindowOpen = false;
let currentViewport = null;
let storedOptions = null;
let refsInvalidatedByNavigation = false;
let lastSnapshotRefCount = 0;

function write(value) {
  process.stdout.write(`${JSON.stringify(value)}\n`);
}

function status(state, message = null) {
  write({
    event: "status",
    sessionId,
    status: {
      sessionId,
      state,
      url: currentUrl,
      message,
      minimized: state === "minimized",
      hasHeadedWindow: !headlessMode && isHeadedWindowOpen,
    },
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

async function importCloakBrowser() {
  const candidates = [
    process.env.JKC_BROWSER_NODE_MODULES,
    join(process.cwd(), "node_modules"),
    join(new URL(".", import.meta.url).pathname, "node_modules"),
  ]
    .filter(Boolean)
    .map((nodeModulesDir) => join(nodeModulesDir, "cloakbrowser", "dist", "index.js"));

  for (const candidate of candidates) {
    if (await fileExists(candidate)) {
      return import(pathToFileURL(candidate).href);
    }
  }

  try {
    return await import("cloakbrowser");
  } catch (error) {
    const searched = candidates.map((path) => `- ${path}`).join("\n");
    throw new Error(
      `无法加载 cloakbrowser ESM 入口：${error instanceof Error ? error.message : String(error)}\n已搜索：\n${searched}`,
    );
  }
}

async function fileExists(path) {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
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
  headlessMode = Boolean(params.headless);
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
    headless: headlessMode,
    humanize: true,
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

  status("launching", "正在启动嵌入式 CloakBrowser");
  context = await cloak.launchPersistentContext(options);
  page = context.pages()[0] || (await context.newPage());
  isHeadedWindowOpen = !headlessMode;
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
  targetPage.on("download", (download) => {
    const suggested = download.suggestedFilename();
    status("downloading", `正在下载：${suggested}`);
    log(`检测到下载：${suggested}`);
  });
  targetPage.on("close", () => {
    status("closed", "浏览器页面已关闭");
    if (!headlessMode) {
      isHeadedWindowOpen = false;
      write({ event: "page_closed", sessionId });
    }
  });
}

async function reopenWindow() {
  const viewport = currentViewport || { width: 1280, height: 800 };

  if (headlessMode) {
    if (!storedOptions) throw new Error("浏览器选项不可用，无法启动独立窗口");
    const reopenUrl = currentUrl;
    try { await context?.close(); } catch { /* ignore */ }
    cleanupCdp();
    context = null;
    page = null;
    headlessMode = false;
    await launchContext(storedOptions, viewport);
    isHeadedWindowOpen = true;
    if (reopenUrl && reopenUrl !== "about:blank") {
      try { await page.goto(reopenUrl, { waitUntil: "domcontentloaded", timeout: 15000 }); } catch { /* keep current */ }
    }
    return { ok: true, url: page.url(), headed: true };
  }

  if (!!page) {
    if (!isHeadedWindowOpen) {
      try { await restoreWindow(); } catch { /* already visible */ }
    }
    try { await page.bringToFront(); } catch { /* ignore */ }
    return { ok: true, url: currentUrl, headed: true };
  }

  if (!context) {
    if (!storedOptions) throw new Error("浏览器上下文已关闭，无法重新打开");
    const reopenUrl = currentUrl;
    try { await context?.close(); } catch { /* ignore */ }
    cleanupCdp();
    context = null;
    await launchContext(storedOptions, viewport);
    isHeadedWindowOpen = true;
    if (reopenUrl && reopenUrl !== "about:blank") {
      try { await page.goto(reopenUrl, { waitUntil: "domcontentloaded", timeout: 15000 }); } catch { /* keep current */ }
    }
    return { ok: true, url: page.url(), headed: true };
  }

  const reopenUrl = currentUrl;
  page = await context.newPage();
  attachPageListeners(page);
  await startScreencast(page, viewport);
  if (reopenUrl && reopenUrl !== "about:blank") {
    try { await page.goto(reopenUrl, { waitUntil: "domcontentloaded", timeout: 15000 }); } catch { /* keep current */ }
  }
  currentUrl = page.url();
  isHeadedWindowOpen = true;
  return { ok: true, url: currentUrl, headed: true };
}

function cleanupCdp() {
  if (cdp) {
    cdp.send("Page.stopScreencast").catch(() => undefined);
    cdp.detach().catch(() => undefined);
    cdp = null;
  }
}

async function focusWindow() {
  if (!cdp || !page) throw new Error("浏览器尚未就绪");
  const { windowId } = await cdp.send("Browser.getWindowForTarget");
  const info = await cdp.send("Browser.getWindowBounds", { windowId });
  const bounds = info.bounds || {};
  const x = Math.max(0, bounds.left ?? 100);
  const y = Math.max(0, bounds.top ?? 100);
  const width = bounds.width ?? 1280;
  const height = bounds.height ?? 800;
  await cdp.send("Browser.setWindowBounds", { windowId, bounds: { left: x, top: y, width, height } });
  await cdp.send("Browser.setWindowBounds", { windowId, bounds: { windowState: "normal" } });
}

async function ensureWindowCdp() {
  if (cdp && page) return cdp;
  if (!page) throw new Error("浏览器页面不可用");
  cdp = await page.context().newCDPSession(page);
  return cdp;
}

async function minimizeWindow() {
  if (headlessMode) throw new Error("当前处于无窗口模式，无需最小化");
  const session = await ensureWindowCdp();
  const { windowId } = await session.send("Browser.getWindowForTarget");
  const info = await session.send("Browser.getWindowBounds", { windowId });
  const bounds = info.bounds || {};
  if ((bounds.left ?? 0) >= 0 && (bounds.top ?? 0) >= 0) {
    savedBounds = { left: bounds.left, top: bounds.top, width: bounds.width, height: bounds.height };
  }
  await session.send("Browser.setWindowBounds", {
    windowId,
    bounds: { left: -32000, top: -32000, width: 1, height: 1 },
  });
  isHeadedWindowOpen = false;
}

async function restoreWindow() {
  if (headlessMode) throw new Error("当前处于无窗口模式，无需恢复");
  const session = await ensureWindowCdp();
  const { windowId } = await session.send("Browser.getWindowForTarget");
  const bounds = savedBounds || { left: 100, top: 100, width: 1280, height: 800 };
  await session.send("Browser.setWindowBounds", {
    windowId,
    bounds: { ...bounds, windowState: "normal" },
  });
  savedBounds = null;
  isHeadedWindowOpen = true;
}

function timeout(params) {
  return Math.max(1, Number(params.timeout || 60000));
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function interactionSettleTimeout(params) {
  return Math.min(1500, Math.max(250, Number(params.settleTimeout || 700)));
}

async function waitForPotentialNavigation(params = {}) {
  const navigated = await page
    .waitForEvent("framenavigated", { timeout: interactionSettleTimeout(params) })
    .then(() => true)
    .catch(() => false);
  if (!navigated) {
    await delay(120);
    currentUrl = page.url();
    return false;
  }
  await page
    .waitForLoadState("domcontentloaded", { timeout: Math.min(timeout(params), 5000) })
    .catch(() => undefined);
  currentUrl = page.url();
  return true;
}

function isLikelyDownloadAbort(error) {
  const message = error instanceof Error ? error.message : String(error);
  return message.includes("net::ERR_ABORTED") || message.includes("Download is starting");
}

function clearElementRefs() {
  elementRefs = new Map();
  // Do NOT reset nextElementRefId — keep globally unique ref IDs
  // so stale refs from a previous snapshot never collide with new ones.
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

function axValue(value) {
  if (!value || value.value === undefined || value.value === null) return "";
  return String(value.value).trim();
}

function createElementRef(node) {
  if (!node.backendDOMNodeId) return "";
  const ref = `r${nextElementRefId++}`;
  elementRefs.set(ref, {
    backendNodeId: node.backendDOMNodeId,
    role: axValue(node.role),
    name: axValue(node.name),
  });
  return ref;
}

function axProperty(node, name) {
  const property = node.properties?.find((item) => item.name === name);
  return axValue(property?.value);
}

function isUsefulAXNode(node) {
  if (!node || node.ignored) return false;
  const role = axValue(node.role);
  const name = axValue(node.name);
  const value = axValue(node.value);
  const description = axValue(node.description);
  return Boolean(role || name || value || description);
}

function formatAXNode(node) {
  const ref = createElementRef(node);
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

function formatAXTree(nodes, params = {}) {
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
  let truncated = false;

  function walk(node, depth) {
    if (!node || visited.has(node.nodeId) || emitted >= maxNodes) {
      if (emitted >= maxNodes) truncated = true;
      return;
    }
    visited.add(node.nodeId);
    if (isUsefulAXNode(node)) {
      lines.push(`${"  ".repeat(depth)}- ${formatAXNode(node)}`);
      emitted += 1;
      if (lines.join("\n").length > maxChars) {
        truncated = true;
        return;
      }
    }
    for (const childId of node.childIds || []) {
      walk(nodeById.get(childId), depth + 1);
      if (truncated) return;
    }
  }

  for (const root of startNodes) {
    walk(root, 0);
    if (truncated) break;
  }

  let text = lines.join("\n").trim();
  if (!text) text = "(Accessibility Tree 为空)";
  if (text.length > maxChars) {
    text = `${text.slice(0, maxChars)}\n...`;
    truncated = true;
  }
  return { text, emitted, truncated };
}

async function readAccessibilitySnapshot(params = {}) {
  status("busy", "正在读取页面可访问性树");
  await cdp.send("Accessibility.enable");
  try {
    let nodes;
    const backendNodeId = params.ref ? refTarget(params.ref).backendNodeId : null;
    clearElementRefs();
    refsInvalidatedByNavigation = false;
    if (backendNodeId) {
      const result = await cdp.send("Accessibility.getPartialAXTree", {
        backendNodeId,
        fetchRelatives: false,
      });
      nodes = result.nodes || [];
    } else {
      const result = await cdp.send("Accessibility.getFullAXTree");
      nodes = result.nodes || [];
    }

    const snapshot = formatAXTree(nodes, params);
    currentUrl = page.url();
    lastSnapshotRefCount = elementRefs.size;
    status("ready", null);
    return {
      text: `# Accessibility Tree Snapshot\nurl: ${currentUrl || ""}\nnode_count: ${nodes.length}\ntruncated: ${snapshot.truncated}\n\n${snapshot.text}`,
      nodeCount: nodes.length,
      emittedNodeCount: snapshot.emitted,
      refCount: elementRefs.size,
      truncated: snapshot.truncated,
      url: currentUrl,
      source: "chrome_devtools_protocol_accessibility",
    };
  } finally {
    await cdp.send("Accessibility.disable").catch(() => undefined);
  }
}

async function elementCenterByRef(ref) {
  const target = refTarget(ref);
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

async function clickElementRef(ref) {
  const point = await elementCenterByRef(ref);
  await page.mouse.click(point.x, point.y);
  return { ref: normalizeRef(ref), x: point.x, y: point.y };
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

async function runWithDownloadFeedback(actionLabel, action, params = {}) {
  const commandTimeout = timeout(params);
  status("busy", actionLabel);
  const downloadPromise = page
    .waitForEvent("download", { timeout: commandTimeout })
    .then(saveDownload)
    .catch((error) => {
      const message = error instanceof Error ? error.message : String(error);
      if (message.includes("Timeout")) return null;
      throw error;
    });

  try {
    const actionResult = await action();
    const downloadResult = await Promise.race([
      downloadPromise,
      new Promise((resolve) => setTimeout(() => resolve(null), 250)),
    ]);
    currentUrl = page.url();
    status("ready", null);
    return downloadResult
      ? { ok: true, ...downloadResult, url: currentUrl }
      : { ok: true, ...actionResult, url: currentUrl };
  } catch (error) {
    if (!isLikelyDownloadAbort(error)) {
      if (page) {
        currentUrl = page.url();
        status("ready", null);
      }
      throw error;
    }
    const downloadResult = await downloadPromise;
    currentUrl = page.url();
    status("ready", null);
    return { ok: true, ...downloadResult, url: currentUrl };
  }
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
    case "minimize_window": {
      await minimizeWindow();
      return { ok: true };
    }
    case "restore_window": {
      await restoreWindow();
      return { ok: true, url: currentUrl };
    }
    case "focus_window": {
      await focusWindow();
      return { ok: true, url: currentUrl };
    }
    case "reopen_window": {
      return reopenWindow();
    }
    case "open_url": {
      if (!params.url) throw new Error("缺少必填参数 url");
      const result = await runWithDownloadFeedback(
        `正在打开：${params.url}`,
        async () => {
          await page.goto(params.url, { waitUntil: "domcontentloaded", timeout: timeout(params) });
          return { title: await page.title() };
        },
        params,
      );
      return result.downloaded ? result : { url: currentUrl, title: result.title };
    }
    case "back": {
      await runWithDownloadFeedback(
        "正在返回上一页",
        async () => {
          await page.goBack({ waitUntil: "domcontentloaded", timeout: timeout(params) });
          return { title: await page.title() };
        },
        params,
      );
      currentUrl = page.url();
      return { ok: true, url: currentUrl, title: await page.title() };
    }
    case "reload": {
      return runWithDownloadFeedback(
        "正在刷新页面",
        async () => {
          await page.reload({ waitUntil: "domcontentloaded", timeout: timeout(params) });
          return { title: await page.title() };
        },
        params,
      );
    }
    case "click": {
      return runWithDownloadFeedback(
        "正在点击页面元素",
        async () => {
          if (params.ref) {
            const point = await elementCenterByRef(params.ref);
            const navigation = waitForPotentialNavigation(params);
            await page.mouse.click(point.x, point.y);
            const navigated = await navigation;
            return { ref: normalizeRef(params.ref), x: point.x, y: point.y, navigated };
          }
          if (Number.isFinite(params.x) && Number.isFinite(params.y)) {
            const navigation = waitForPotentialNavigation(params);
            await page.mouse.click(Number(params.x), Number(params.y));
            const navigated = await navigation;
            return { navigated };
          } else {
            throw new Error("browser_click 需要 ref；请先调用 browser_read_text 获取元素 ref");
          }
        },
        params,
      );
    }
    case "type": {
      if (!params.ref) throw new Error("缺少必填参数 ref；请先调用 browser_read_text 获取输入元素 ref");
      if (typeof params.text !== "string") throw new Error("缺少必填参数 text");
      await clickElementRef(params.ref);
      await page.keyboard.type(params.text, { delay: 35 });
      return { ok: true, ref: normalizeRef(params.ref) };
    }
    case "press": {
      if (!params.key) throw new Error("缺少必填参数 key");
      return runWithDownloadFeedback(
        `正在发送按键：${params.key}`,
        async () => {
          await page.keyboard.press(params.key);
          return {};
        },
        params,
      );
    }
    case "wait_for": {
      const state = params.loadState || "domcontentloaded";
      await page.waitForLoadState(state, { timeout: timeout(params) });
      return { ok: true, loadState: state };
    }
    case "read_text": {
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
status("booting", "CloakBrowser sidecar 已启动");

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

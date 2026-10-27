/**
 * 普通聊天文件区的目录选择纯逻辑：默认会话推导、可选根列表构造与
 * 默认根推导。组件层只做状态持有与渲染，便于单测。
 */
import type { ChatSession, ChatWorkspaceOverview } from "../../types";

/** 面板可选的浏览根：分类工程目录（可能失效）或会话临时目录。 */
export interface ChatFileRoot {
  key: string;
  kind: "project" | "session";
  path: string;
  /** 工程目录失效（不可浏览）时的原因；会话目录恒可选中（未创建仅提示）。 */
  error: string | null;
}

/** 默认临时目录会话：优先文件入口选中的会话（若属于该分类），否则取列表首个。 */
export function pickDefaultSessionId(
  preferredSessionId: string | null,
  sessions: Pick<ChatSession, "id">[],
): string | null {
  if (preferredSessionId && sessions.some((session) => session.id === preferredSessionId)) {
    return preferredSessionId;
  }
  return sessions[0]?.id ?? null;
}

/** 路径最后一段（工程目录行的主标签）；根路径或空串回退整串。 */
export function pathBaseName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const separatorIndex = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return separatorIndex === -1 ? trimmed : trimmed.slice(separatorIndex + 1);
}

/** 由总览构造可选择的根列表：工程目录在前（失效项保留供展示原因），会话目录殿后。 */
export function buildChatFileRoots(overview: ChatWorkspaceOverview | undefined): ChatFileRoot[] {
  if (!overview) {
    return [];
  }
  const roots: ChatFileRoot[] = overview.projectDirs.map((dir) => ({
    key: `project:${dir.raw}`,
    kind: "project" as const,
    // 失效目录不可浏览，但保留 raw 供展示；path 仅在有效时被选中。
    path: dir.canonical ?? dir.raw,
    error: dir.error,
  }));
  if (overview.sessionDir) {
    roots.push({
      key: "session",
      kind: "session",
      path: overview.sessionDir,
      error: null,
    });
  }
  return roots;
}

/** 默认浏览根：首个有效工程目录；都没有则取会话临时目录。 */
export function pickInitialRoot(roots: ChatFileRoot[]): string | null {
  return roots.find((root) => !root.error)?.path ?? null;
}

/* ── 路径显示缩写 ──────────────────────────────────────────────
   侧栏宽度只有 200-480px，绝对路径原样展示会把名称挤没；展示层统一走
   这三个纯函数：home → `~`、超长中间省略、面包屑拆成「名称 + 父目录」。 */

/** 应用数据目录固定在 `~/.jkcodingagent`，用它作推断 home 的锚点。 */
const HOME_ANCHOR_RE = /^(.*?)[\\/]\.jkcodingagent(?:[\\/]|$)/;

/**
 * 从一批路径里推断用户主目录：定位 `{home}/.jkcodingagent` 片段。
 *
 * 前端没有可信的 home 来源（`@tauri-apps/api/path` 的 `homeDir` 未在
 * capabilities 授权），但会话临时目录恒为 `~/.jkcodingagent/plain-chat-browser/…`，
 * 足以反推 home 前缀；推断失败（无会话目录/结构变了）时返回 null，
 * 调用方退回原样绝对路径，不会显示错误的缩写。
 */
export function inferHomeDir(paths: Array<string | null | undefined>): string | null {
  for (const path of paths) {
    if (!path) continue;
    const match = HOME_ANCHOR_RE.exec(path);
    const home = match?.[1] ?? "";
    // `''` 与纯分隔符（`/`）都不算 home，避免把绝对路径缩成 `~/…`。
    if (home !== "" && !/^[\\/]+$/.test(home)) return home;
  }
  return null;
}

/** home 前缀缩写为 `~`；路径与 home 相等时只剩 `~`，其余原样返回。 */
export function abbreviateHomePath(path: string, homeDir: string | null): string {
  if (!path || !homeDir) return path;
  const home = homeDir.replace(/[\\/]+$/, "");
  if (home === "") return path;
  if (path === home) return "~";
  if (path.startsWith(`${home}/`) || path.startsWith(`${home}\\`)) {
    const rest = path.slice(home.length);
    // 只有末尾分隔符（`{home}/`）时也收成 `~`，避免出现悬空的 `~/`。
    return /^[\\/]+$/.test(rest) ? "~" : `~${rest}`;
  }
  return path;
}

/** 超过 `maxChars`（按码点计）时保留头尾、中间用 `…` 省略，结果长度恰为 `maxChars`。 */
export function middleEllipsis(text: string, maxChars: number): string {
  if (maxChars <= 0) return "";
  if (maxChars === 1) return text.length <= 1 ? text : "…";
  const chars = [...text];
  if (chars.length <= maxChars) return text;
  const keep = maxChars - 1;
  const head = Math.ceil(keep / 2);
  const tail = keep - head;
  const tailPart = tail > 0 ? chars.slice(-tail).join("") : "";
  return `${chars.slice(0, head).join("")}…${tailPart}`;
}

/** 侧栏展示用路径：home 前缀缩成 `~`，再按字符预算做中间省略。 */
export function formatDisplayPath(
  path: string,
  options: { homeDir?: string | null; maxChars?: number } = {},
): string {
  const { homeDir = null, maxChars = 30 } = options;
  return middleEllipsis(abbreviateHomePath(path, homeDir), maxChars);
}

/**
 * 面包屑拆分：末段是主名称，父目录是 muted 路径（已缩写）。
 *
 * `~/work/java/deploy-console` → `{ name: "deploy-console", parent: "~/work/java" }`。
 * 名称单独渲染可保证根目录名永不被路径挤压；根路径 `/` 无父目录。
 */
export function splitPathDisplay(
  path: string,
  options: { homeDir?: string | null; maxChars?: number } = {},
): { name: string; parent: string } {
  const name = pathBaseName(path) || path;
  const trimmed = path.replace(/[\\/]+$/, "");
  const separatorIndex = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  const parentPath = separatorIndex > 0 ? trimmed.slice(0, separatorIndex) : "";
  return {
    name,
    parent: parentPath ? formatDisplayPath(parentPath, options) : "",
  };
}

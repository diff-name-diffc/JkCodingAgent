import { resolveFilePresentation } from "./file-icons";

export const AVATAR_COLORS: [string, string][] = [
  ["#3B82F6", "#1D4ED8"],
  ["#6366F1", "#4338CA"],
  ["#8B5CF6", "#6D28D9"],
  ["#A855F7", "#7E22CE"],
  ["#06B6D4", "#0E7490"],
  ["#14B8A6", "#0F766E"],
  ["#0EA5E9", "#0369A1"],
  ["#10B981", "#047857"],
  ["#818CF8", "#4F46E5"],
  ["#22D3EE", "#0891B2"],
];

export function getAvatarGradient(name: string): [string, string] {
  let hash = 0;
  for (let i = 0; i < name.length; i++) hash = (hash * 31 + name.charCodeAt(i)) & 0xffffffff;
  return AVATAR_COLORS[Math.abs(hash) % AVATAR_COLORS.length];
}

export function shortenPath(p: string) {
  return p.replace(/^\/Users\/[^/]+/, "~");
}

export function load<T>(key: string, fallback: T): T {
  try {
    const r = localStorage.getItem(key);
    return r ? JSON.parse(r) : fallback;
  } catch {
    return fallback;
  }
}
export function save<T>(key: string, val: T) {
  localStorage.setItem(key, JSON.stringify(val));
}

type ImeKeyboardEvent = {
  key?: string;
  keyCode?: number;
  which?: number;
  nativeEvent?: {
    isComposing?: boolean;
    keyCode?: number;
    which?: number;
  };
};

// macOS 中文输入法在确认候选时，部分场景下会把 Enter 暴露成 keyCode 229 / Process。
export function isImeComposing(event: ImeKeyboardEvent): boolean {
  return Boolean(
    event.nativeEvent?.isComposing ||
      event.nativeEvent?.keyCode === 229 ||
      event.nativeEvent?.which === 229 ||
      event.keyCode === 229 ||
      event.which === 229 ||
      event.key === "Process",
  );
}

// ── Usage 颜色工具 ────────────────────────────────────────────────────────────

export function getUsageColor(remainingPercent: number): string {
  if (remainingPercent > 70) return "var(--usage-good)";
  if (remainingPercent >= 20) return "var(--usage-warn)";
  return "var(--usage-danger)";
}

// ── Git 状态工具 ──────────────────────────────────────────────────────────────

export function getGitStatusColor(status: string): string {
  switch (status) {
    case "A":
      return "#3fb950";
    case "D":
      return "#f85149";
    case "M":
      return "#e3b341";
    case "R":
      return "#79c0ff";
    case "?":
      return "#79c0ff";
    case "U":
      return "#f85149";
    default:
      return "var(--text-muted)";
  }
}

export function getGitStatusLabel(status: string): string {
  switch (status) {
    case "A":
      return "A";
    case "D":
      return "D";
    case "M":
      return "M";
    case "R":
      return "R";
    case "?":
      return "U";
    case "U":
      return "!";
    default:
      return status;
  }
}

// ── 文件颜色工具 ──────────────────────────────────────────────────────────────

export function getFileColor(name: string, ext?: string): string {
  return resolveFilePresentation({ name, extension: ext }).accentColor;
}

// ── 文件类型扩展名集合 ────────────────────────────────────────────────────────

export const CODE_EXTS = new Set([
  "ts",
  "tsx",
  "mts",
  "cts",
  "js",
  "jsx",
  "mjs",
  "cjs",
  "rs",
  "py",
  "go",
  "java",
  "kt",
  "kts",
  "swift",
  "rb",
  "php",
  "cs",
  "c",
  "cpp",
  "h",
  "hpp",
  "css",
  "html",
  "vue",
  "svelte",
  "astro",
  "lua",
  "sql",
  "graphql",
  "gql",
  "proto",
]);

// ── Git 文件路径工具 ──────────────────────────────────────────────────────────

export function fileName(path: string): string {
  return path.split("/").pop() ?? path;
}

export function fileDir(path: string): string {
  const parts = path.split("/");
  return parts.length > 1 ? parts.slice(0, -1).join("/") : "";
}

/**
 * 文件树剪贴板动作（⌘C / ⌘V）的纯逻辑：快捷键识别、粘贴落点推导、提示文案。
 * 组件层只负责调用后端命令与刷新树，便于用 Vitest 覆盖这些判断。
 */

import { getPathDirectory } from "../../utils/filePaths";

/** 剪贴板动作只需要节点的路径与类型，避免把整棵 TreeNode 拖进纯函数。 */
export interface ClipboardNodeRef {
  path: string;
  is_dir: boolean;
}

export type ClipboardShortcut = "copy" | "paste";

/** KeyboardEvent 的最小投影（Vitest 里无需构造真实事件）。 */
export interface ShortcutEventLike {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
}

/** 只认 ⌘/Ctrl + C/V；带 Shift/Alt 的组合键（如 ⌘⇧C）交给其他功能。 */
export function resolveClipboardShortcut(event: ShortcutEventLike): ClipboardShortcut | null {
  if (!event.metaKey && !event.ctrlKey) {
    return null;
  }
  if (event.altKey || event.shiftKey) {
    return null;
  }

  switch (event.key.toLowerCase()) {
    case "c":
      return "copy";
    case "v":
      return "paste";
    default:
      return null;
  }
}

function trimTrailingSeparators(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  if (!trimmed) {
    // 根路径（"/" 或 "C:\"）整体被剥光时保留原值，避免退化成空串。
    return path;
  }
  // 盘符根（"C:\" / "C:/")剥离后会退化成 "C:"——Windows 上表示
  // 「C 盘当前目录」的相对引用，不能作为落点，把分隔符补回来。
  if (/^[A-Za-z]:$/.test(trimmed)) {
    return `${trimmed}${path.slice(-1)}`;
  }
  return trimmed;
}

/** 取路径最后一段作为展示名（兼容 / 与 \ 分隔符）。 */
export function clipboardEntryName(path: string): string {
  const trimmed = trimTrailingSeparators(path);
  const separatorIndex = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return separatorIndex === -1 ? trimmed : trimmed.slice(separatorIndex + 1);
}

/**
 * ⌘V 的落点目录：选中目录 → 目录本身；选中文件 → 其父目录；未选中 → 树的根。
 */
export function resolvePasteDirectory(target: ClipboardNodeRef | null, rootPath: string): string {
  if (!target) {
    return rootPath;
  }
  if (target.is_dir) {
    return target.path;
  }

  const parent = trimTrailingSeparators(getPathDirectory(target.path));
  return parent && parent !== "." ? parent : rootPath;
}

/** 复制成功提示：单条带名字与用途说明，多条只报数量。 */
export function describeCopiedEntries(entries: readonly ClipboardNodeRef[]): string {
  if (entries.length === 0) {
    return "已复制到剪贴板";
  }
  if (entries.length === 1) {
    const entry = entries[0];
    const noun = entry.is_dir ? "目录" : "文件";
    return `已复制${noun}「${clipboardEntryName(entry.path)}」· 可在访达或其他应用中粘贴`;
  }
  return `已复制 ${entries.length} 项 · 可在访达或其他应用中粘贴`;
}

/** 粘贴成功提示：单条带名字（后端可能因重名生成「副本」名，用真实结果）。 */
export function describePastedEntries(paths: readonly string[]): string {
  if (paths.length === 0) {
    return "已粘贴";
  }
  if (paths.length === 1) {
    return `已粘贴「${clipboardEntryName(paths[0])}」`;
  }
  return `已粘贴 ${paths.length} 项`;
}

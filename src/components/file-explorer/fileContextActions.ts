import type { LucideIcon } from "lucide-react";
import { AtSign, ClipboardCopy, ClipboardPaste, Copy, PencilLine, Trash2 } from "lucide-react";
import { isMacPlatform } from "../../lib/keyboard-bindings";
import type { TreeNode } from "./tree";

/** 菜单文案里的 Mod 键写法（macOS 用 ⌘，其余 Ctrl+），与快捷键判定同源。 */
const MOD_KEY = isMacPlatform() ? "⌘" : "Ctrl+";

type FileContextActionItem = {
  id: string;
  label: string;
  caption?: string;
  icon: LucideIcon;
  tone?: "default" | "danger";
  onSelect: () => void | Promise<void>;
};

type FileContextActionGroup = {
  id: string;
  label?: string;
  items: FileContextActionItem[];
};

export function buildFileContextActionGroups({
  node,
  relativePath,
  onCopyPath,
  onCopyMentionPath,
  onCopyEntry,
  onPasteEntry,
  onRename,
  onDelete,
}: {
  node: TreeNode;
  relativePath: string;
  onCopyPath: () => void | Promise<void>;
  onCopyMentionPath: () => void | Promise<void>;
  /** 复制文件本体到系统剪贴板（可在访达/其他应用中粘贴）。 */
  onCopyEntry: () => void | Promise<void>;
  /** 把剪贴板中的文件粘贴到当前落点目录。 */
  onPasteEntry: () => void | Promise<void>;
  onRename: () => void;
  onDelete: () => void | Promise<void>;
}): FileContextActionGroup[] {
  const entryLabel = node.is_dir ? "目录" : "文件";

  return [
    {
      id: "clipboard",
      label: "剪贴板",
      items: [
        {
          id: "copy-entry",
          label: `复制${entryLabel}`,
          caption: `${MOD_KEY}C · 可在访达或其他应用中粘贴`,
          icon: ClipboardCopy,
          onSelect: onCopyEntry,
        },
        {
          id: "paste-entry",
          label: "粘贴到此处",
          caption: `${MOD_KEY}V · 复制剪贴板中的文件`,
          icon: ClipboardPaste,
          onSelect: onPasteEntry,
        },
      ],
    },
    {
      id: "edit",
      label: "编辑",
      items: [
        {
          id: "rename",
          label: "重命名",
          caption: `修改${entryLabel}名称`,
          icon: PencilLine,
          onSelect: onRename,
        },
        {
          id: "delete",
          label: "删除",
          caption: `永久删除${entryLabel}`,
          icon: Trash2,
          tone: "danger",
          onSelect: onDelete,
        },
      ],
    },
    {
      id: "path",
      label: "路径",
      items: [
        {
          id: "copy-path",
          label: "复制路径",
          caption: relativePath,
          icon: Copy,
          onSelect: onCopyPath,
        },
        {
          id: "copy-mention-path",
          label: "复制 @路径",
          caption: "用于提示词 @ 提及",
          icon: AtSign,
          onSelect: onCopyMentionPath,
        },
      ],
    },
  ];
}

export type { FileContextActionGroup, FileContextActionItem };

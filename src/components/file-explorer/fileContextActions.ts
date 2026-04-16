import type { LucideIcon } from "lucide-react";
import { AtSign, Copy, PencilLine, Trash2 } from "lucide-react";
import type { TreeNode } from "./tree";

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
  onRename,
  onDelete,
}: {
  node: TreeNode;
  relativePath: string;
  onCopyPath: () => void | Promise<void>;
  onCopyMentionPath: () => void | Promise<void>;
  onRename: () => void;
  onDelete: () => void | Promise<void>;
}): FileContextActionGroup[] {
  const entryLabel = node.is_dir ? "目录" : "文件";

  return [
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

import * as ContextMenu from "@radix-ui/react-context-menu";
import { Pencil, Trash2 } from "lucide-react";
import type React from "react";
import type { ChatCategory } from "../types";

interface ChatCategoryContextMenuProps {
  category: ChatCategory;
  children: React.ReactNode;
  onRename: () => void;
  onDelete: () => void;
}

export function ChatCategoryContextMenu({
  children,
  onRename,
  onDelete,
}: ChatCategoryContextMenuProps) {
  return (
    <ContextMenu.Root>
      <ContextMenu.Trigger asChild>{children}</ContextMenu.Trigger>
      <ContextMenu.Portal>
        <ContextMenu.Content className="ai-context-menu">
          <ContextMenu.Item
            onSelect={onRename}
            className="ai-context-menu-item"
          >
            <Pencil size={13} /> 重命名
          </ContextMenu.Item>
          <ContextMenu.Separator className="ai-context-menu-separator" />
          <ContextMenu.Item
            onSelect={onDelete}
            className="ai-context-menu-item ai-context-menu-item-danger"
          >
            <Trash2 size={13} /> 删除分类
          </ContextMenu.Item>
        </ContextMenu.Content>
      </ContextMenu.Portal>
    </ContextMenu.Root>
  );
}

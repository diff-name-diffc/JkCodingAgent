import * as ContextMenu from "@radix-ui/react-context-menu";
import { Pencil, Trash2 } from "lucide-react";
import type { ChatCategory } from "../types";

interface ChatCategoryContextMenuProps {
  category: ChatCategory;
  children: React.ReactNode;
  onRename: () => void;
  onDelete: () => void;
}

const menuItemStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  padding: "7px 10px",
  fontSize: 12.5,
  cursor: "pointer",
  color: "var(--text-primary)",
  backgroundColor: "var(--bg-card)",
  borderRadius: 4,
  outline: "none",
};

const menuItemHoverStyle: React.CSSProperties = {
  backgroundColor: "var(--bg-hover)",
  color: "var(--text-primary)",
};

export function ChatCategoryContextMenu({
  children,
  onRename,
  onDelete,
}: ChatCategoryContextMenuProps) {
  return (
    <ContextMenu.Root>
      <ContextMenu.Trigger asChild>{children}</ContextMenu.Trigger>
      <ContextMenu.Portal>
        <ContextMenu.Content
          style={{
            minWidth: 120,
            padding: 4,
            background: "var(--bg-card)",
            border: "1px solid var(--border-medium)",
            borderRadius: 8,
            boxShadow: "0 8px 24px rgba(0,0,0,0.22)",
            outline: "none",
            zIndex: 50,
          }}
        >
          <ContextMenu.Item
            onSelect={onRename}
            style={menuItemStyle}
            onMouseEnter={(e) => Object.assign(e.currentTarget.style, menuItemHoverStyle)}
            onMouseLeave={(e) => Object.assign(e.currentTarget.style, { backgroundColor: "var(--bg-card)" })}
          >
            <Pencil size={13} /> 重命名
          </ContextMenu.Item>
          <ContextMenu.Separator style={{ height: 1, margin: "4px 0", background: "var(--border-dim)" }} />
          <ContextMenu.Item
            onSelect={onDelete}
            style={menuItemStyle}
            onMouseEnter={(e) => Object.assign(e.currentTarget.style, menuItemHoverStyle)}
            onMouseLeave={(e) => Object.assign(e.currentTarget.style, { backgroundColor: "var(--bg-card)" })}
          >
            <Trash2 size={13} color="var(--text-destructive, #EF4444)" /> 删除分类
          </ContextMenu.Item>
        </ContextMenu.Content>
      </ContextMenu.Portal>
    </ContextMenu.Root>
  );
}

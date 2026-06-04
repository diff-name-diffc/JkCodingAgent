import { ChevronRight, Plus } from "lucide-react";
import { memo, useState } from "react";
import {
  MessageSquare,
  Heart,
  Briefcase,
  Code2,
  GraduationCap,
  Folder,
  Inbox,
} from "lucide-react";
import type { ChatCategory, ChatSession } from "../types";
import { ChatSessionCard } from "./ChatSessionCard";
import { ChatCategoryContextMenu } from "./ChatCategoryContextMenu";
import s from "../styles";

const CATEGORY_ICON_MAP: Record<string, React.ElementType> = {
  MessageSquare,
  Heart,
  Briefcase,
  Code2,
  GraduationCap,
  Folder,
  Inbox,
};

function resolveIcon(iconName: string): React.ElementType {
  return CATEGORY_ICON_MAP[iconName] ?? Folder;
}

interface ChatCategorySectionProps {
  category: ChatCategory | null;
  sessions: ChatSession[];
  activeSessionId: string | null;
  runningSessionIds: Set<string>;
  onToggle: () => void;
  isExpanded: boolean;
  onSessionClick: (id: string) => void;
  onSessionDelete: (id: string, e: React.MouseEvent) => void;
  onSessionDragStart: (sessionId: string, e: React.DragEvent) => void;
  onNewInCategory: () => void;
  onRenameCategory: () => void;
  onDeleteCategory: () => void;
  onDragOver: (e: React.DragEvent) => void;
  onDrop: (e: React.DragEvent) => void;
  dragOverId: string | null;
}

const UNCATEGORIZED_LABEL = "未分类";

export const ChatCategorySection = memo(function ChatCategorySection({
  category,
  sessions,
  activeSessionId,
  runningSessionIds,
  onToggle,
  isExpanded,
  onSessionClick,
  onSessionDelete,
  onSessionDragStart,
  onNewInCategory,
  onRenameCategory,
  onDeleteCategory,
  onDragOver,
  onDrop,
  dragOverId,
}: ChatCategorySectionProps) {
  const [isHovering, setIsHovering] = useState(false);
  const categoryId = category?.id ?? "__uncategorized__";
  const displayName = category?.name ?? UNCATEGORIZED_LABEL;
  const isUncategorized = !category;
  const Icon = category ? resolveIcon(category.icon) : Inbox;
  const iconColor = category?.color || "var(--text-muted)";

  const chevronTransform = isExpanded ? "rotate(90deg)" : "rotate(0deg)";

  const isDragTarget = dragOverId === categoryId;

  const sharedHeaderProps = {
    style: {
      ...s.categoryHeader,
      background: isHovering ? "var(--bg-hover)" : "transparent",
      cursor: "pointer",
    } as React.CSSProperties,
    onMouseEnter: () => setIsHovering(true),
    onMouseLeave: () => setIsHovering(false),
    onDragOver: (e: React.DragEvent) => {
      e.preventDefault();
      e.stopPropagation();
      onDragOver(e);
    },
    onDrop,
  };

  const headerClickArea = (
    <>
      <ChevronRight
        size={12}
        style={{ ...(s.categoryChevron as React.CSSProperties), transform: chevronTransform }}
      />
      <Icon size={13} style={{ ...(s.categoryIcon as React.CSSProperties), color: iconColor }} />
      <span style={s.categoryName}>{displayName}</span>
      <span style={s.categoryCount}>{sessions.length}</span>
    </>
  );

  const outerHeader = isUncategorized ? (
    // No context menu — render directly; whole row is clickable
    <div {...sharedHeaderProps} onClick={onToggle}>
      {headerClickArea}
    </div>
  ) : (
    // Outer row: trigger covers chevron+icon+name+count; `+` button is a sibling outside trigger
    <div {...sharedHeaderProps}>
      <ChatCategoryContextMenu
        category={category!}
        onRename={onRenameCategory}
        onDelete={onDeleteCategory}
      >
        <div onClick={onToggle} style={{ display: "contents" }}>
          {headerClickArea}
        </div>
      </ChatCategoryContextMenu>
      <button
        style={{
          ...(s.categoryActionBtn as React.CSSProperties),
          opacity: isHovering ? 0.7 : 0,
        }}
        onClick={(e) => {
          e.stopPropagation();
          onNewInCategory();
        }}
        title="在此分类新建聊天"
        onMouseEnter={(ev) => (ev.currentTarget.style.opacity = "1")}
        onMouseLeave={(ev) =>
          (ev.currentTarget.style.opacity = isHovering ? "0.7" : "0")
        }
      >
        <Plus size={12} />
      </button>
    </div>
  );

  const body = isExpanded ? (
    <div
      style={{
        ...(s.categoryChildren as React.CSSProperties),
        ...(isDragTarget ? (s.sessionDragOver as React.CSSProperties) : {}),
      }}
      onDragOver={(e) => {
        e.preventDefault();
        e.stopPropagation();
        onDragOver(e);
      }}
      onDrop={onDrop}
    >
      {sessions.map((session) => (
        <ChatSessionCard
          key={session.id}
          session={session}
          isActive={activeSessionId === session.id}
          isRunning={runningSessionIds.has(session.id)}
          onClick={() => onSessionClick(session.id)}
          onDelete={(e) => onSessionDelete(session.id, e)}
          onDragStart={(e) => onSessionDragStart(session.id, e)}
        />
      ))}
      {sessions.length === 0 && (
        <div
          style={{
            padding: "10px 24px 10px 32px",
            fontSize: 11.5,
            color: "var(--text-hint)",
          }}
        >
          暂无聊天
        </div>
      )}
    </div>
  ) : null;

  if (isUncategorized) {
    if (sessions.length === 0) return null;
    return (
      <div style={s.categorySection as React.CSSProperties}>
        {outerHeader}
        {body}
      </div>
    );
  }

  return (
    <div style={s.categorySection as React.CSSProperties}>
      {outerHeader}
      {body}
    </div>
  );
});

import { memo } from "react";
import { LoaderCircle, Trash2 } from "lucide-react";
import type { DispatcherSession } from "../types";
import s from "../styles";

function formatTime(timestampStr: string) {
  try {
    const d = new Date(timestampStr);
    return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  } catch {
    return timestampStr;
  }
}

interface ChatSessionCardProps {
  session: DispatcherSession;
  isActive: boolean;
  isRunning: boolean;
  onClick: () => void;
  onDelete: (e: React.MouseEvent) => void;
  onDragStart: (e: React.DragEvent) => void;
}

export const ChatSessionCard = memo(function ChatSessionCard({
  session,
  isActive,
  isRunning,
  onClick,
  onDelete,
  onDragStart,
}: ChatSessionCardProps) {
  return (
    <div
      key={session.id}
      draggable
      onDragStart={onDragStart}
      onClick={onClick}
      style={{
        ...s.sessionCard,
        background: isActive ? "var(--bg-selected)" : "transparent",
      }}
    >
      <div style={s.sessionCardBody as React.CSSProperties}>
        <div style={s.sessionCardTitle}>{session.title}</div>
        <div style={s.sessionCardSub}>{formatTime(session.updatedAt)}</div>
      </div>
      <div style={s.sessionCardActions as React.CSSProperties}>
        {isRunning && (
          <LoaderCircle size={12} className="spin" style={{ color: "var(--accent)", opacity: 0.85 }} />
        )}
        <button
          style={{
            ...s.taskDeleteBtn,
            background: "none",
            border: "none",
            cursor: "pointer",
            padding: 2,
            display: "flex",
            alignItems: "center",
          }}
          onClick={onDelete}
          title="删除聊天"
        >
          <Trash2 size={12} color="var(--text-muted)" />
        </button>
      </div>
    </div>
  );
});

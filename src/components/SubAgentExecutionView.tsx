import { useState } from "react";
import { Bot, ChevronDown, ChevronRight, LoaderCircle, XCircle, Check } from "lucide-react";
import type { SubAgentSession } from "./subAgentEventStore";

interface SubAgentExecutionCardProps {
  session: SubAgentSession;
  autoExpand?: boolean;
}

export function SubAgentExecutionCard({ session, autoExpand = true }: SubAgentExecutionCardProps) {
  const [isOpen, setIsOpen] = useState(autoExpand);

  const Icon =
    session.status === "running"
      ? LoaderCircle
      : session.status === "failed"
        ? XCircle
        : Check;
  const iconColor =
    session.status === "running"
      ? "var(--accent)"
      : session.status === "failed"
        ? "var(--danger, #ef4444)"
        : "var(--success, #22c55e)";

  return (
    <div
      style={{
        border: "1px solid var(--border-medium)",
        borderRadius: 10,
        background: "var(--bg-subtle)",
        overflow: "hidden",
      }}
    >
      <button
        type="button"
        onClick={() => setIsOpen((prev) => !prev)}
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          width: "100%",
          padding: "8px 12px",
          background: "transparent",
          border: "none",
          color: "var(--text-primary)",
          cursor: "pointer",
          textAlign: "left",
        }}
      >
        {isOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        <Bot size={14} style={{ color: iconColor }} />
        <span style={{ fontSize: 12, fontWeight: 700 }}>
          子智能体：{session.name}
        </span>
        <span
          style={{
            fontSize: 11,
            color: iconColor,
            marginLeft: "auto",
            display: "flex",
            alignItems: "center",
            gap: 4,
          }}
        >
          <Icon size={12} className={session.status === "running" ? "spin" : ""} />
          {session.status === "running"
            ? `执行中 (${Math.round(session.elapsed / 1000)}s)`
            : session.status === "failed"
              ? "已失败"
              : "已完成"}
        </span>
      </button>
      {isOpen && (
        <div
          style={{
            padding: "0 12px 8px",
            fontSize: 11,
            lineHeight: 1.6,
            color: "var(--text-secondary)",
          }}
        >
          {session.task && (
            <div style={{ color: "var(--text-muted)", marginBottom: 4 }}>
              任务：{session.task.slice(0, 200)}
              {session.task.length > 200 ? "..." : ""}
            </div>
          )}
          <div
            style={{
              display: "flex",
              flexDirection: "column",
              gap: 2,
              maxHeight: 160,
              overflowY: "auto",
            }}
          >
            {session.events.map((ev) => (
              <div
                key={ev.id}
                style={{
                  padding: "1px 4px",
                  fontFamily: "var(--font-mono)",
                  wordBreak: "break-all",
                  whiteSpace: "pre-wrap",
                }}
              >
                {ev.text}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

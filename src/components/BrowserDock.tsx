import { useCallback, useState } from "react";
import type { CSSProperties } from "react";
import { Globe, X } from "lucide-react";

export interface DockedBrowser {
  sessionId: string;
  url: string | null;
  state: string;
}

interface Props {
  sessions: DockedBrowser[];
  onRestore: (sessionId: string) => void;
  onClose: (sessionId: string) => void;
}

function shortLabel(url: string | null, sessionId: string): string {
  if (url && url !== "about:blank") {
    try {
      const host = new URL(url).hostname;
      return host || sessionId.slice(0, 4);
    } catch {
      return sessionId.slice(0, 4);
    }
  }
  return sessionId.slice(0, 4) || "?";
}

function dockedIconColor(sessionId: string): string {
  let hash = 0;
  for (let i = 0; i < sessionId.length; i++) {
    hash = (hash * 31 + sessionId.charCodeAt(i)) | 0;
  }
  const hue = Math.abs(hash) % 360;
  return `hsl(${hue}, 55%, 55%)`;
}

export function BrowserDock({ sessions, onRestore, onClose }: Props) {
  const [hoveredId, setHoveredId] = useState<string | null>(null);

  const handleRestore = useCallback(
    (sessionId: string) => onRestore(sessionId),
    [onRestore],
  );

  const handleClose = useCallback(
    (e: React.MouseEvent, sessionId: string) => {
      e.stopPropagation();
      onClose(sessionId);
    },
    [onClose],
  );

  if (sessions.length === 0) return null;

  return (
    <div style={dockContainer}>
      {sessions.map((session) => {
        const isHovered = hoveredId === session.sessionId;
        const label = shortLabel(session.url, session.sessionId);
        const color = dockedIconColor(session.sessionId);
        return (
          <div
            key={session.sessionId}
            style={{ ...dockItem, background: isHovered ? "var(--bg-card)" : "var(--bg-sidebar)" }}
            onMouseEnter={() => setHoveredId(session.sessionId)}
            onMouseLeave={() => setHoveredId(null)}
            onClick={() => handleRestore(session.sessionId)}
            title={session.url || session.sessionId}
          >
            <div style={{ ...iconCircle, borderColor: color }}>
              <Globe size={14} style={{ color }} />
            </div>
            <span style={dockLabel}>{label}</span>
            {isHovered && (
              <button
                type="button"
                style={closeBtn}
                onClick={(e) => handleClose(e, session.sessionId)}
                title="完全关闭"
              >
                <X size={10} />
              </button>
            )}
          </div>
        );
      })}
    </div>
  );
}

const dockContainer: CSSProperties = {
  position: "absolute",
  bottom: 12,
  right: 56,
  display: "flex",
  gap: 6,
  zIndex: 40,
  padding: "4px 6px",
  background: "var(--bg-root)",
  border: "1px solid var(--border-dim)",
  borderRadius: 10,
  boxShadow: "0 4px 16px rgba(0,0,0,0.18)",
};

const dockItem: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 6,
  padding: "5px 8px",
  borderRadius: 8,
  cursor: "pointer",
  border: "1px solid var(--border-dim)",
  transition: "background 0.12s",
};

const iconCircle: CSSProperties = {
  width: 22,
  height: 22,
  borderRadius: "50%",
  border: "1.5px solid",
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  flexShrink: 0,
};

const dockLabel: CSSProperties = {
  fontSize: 11,
  color: "var(--text-secondary)",
  whiteSpace: "nowrap",
  maxWidth: 80,
  overflow: "hidden",
  textOverflow: "ellipsis",
};

const closeBtn: CSSProperties = {
  width: 16,
  height: 16,
  borderRadius: "50%",
  border: "none",
  background: "var(--bg-sidebar)",
  color: "var(--text-hint)",
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  cursor: "pointer",
  padding: 0,
  flexShrink: 0,
};

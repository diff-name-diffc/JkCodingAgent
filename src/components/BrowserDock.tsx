import { useCallback, useState } from "react";
import { Globe, X } from "lucide-react";

export interface DockedBrowser {
  sessionId: string;
  url: string | null;
  state: string;
}

interface Props {
  sessions: DockedBrowser[];
  onRestore: (sessionId: string) => void | Promise<void>;
  onClose: (sessionId: string) => void | Promise<void>;
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

  const handleRestore = useCallback((sessionId: string) => void onRestore(sessionId), [onRestore]);

  const handleClose = useCallback(
    (e: React.MouseEvent, sessionId: string) => {
      e.stopPropagation();
      void onClose(sessionId);
    },
    [onClose],
  );

  if (sessions.length === 0) return null;

  return (
    <div className="ai-browser-dock">
      {sessions.map((session) => {
        const isHovered = hoveredId === session.sessionId;
        const label = shortLabel(session.url, session.sessionId);
        const color = dockedIconColor(session.sessionId);
        return (
          <div
            key={session.sessionId}
            className={isHovered ? "ai-browser-dock-item is-hovered" : "ai-browser-dock-item"}
            onMouseEnter={() => setHoveredId(session.sessionId)}
            onMouseLeave={() => setHoveredId(null)}
            onClick={() => handleRestore(session.sessionId)}
            title={session.url || session.sessionId}
          >
            <div className="ai-browser-dock-icon" style={{ borderColor: color }}>
              <Globe size={14} style={{ color }} />
            </div>
            <span className="ai-browser-dock-label">{label}</span>
            {isHovered && (
              <button
                type="button"
                className="ai-browser-dock-close"
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

import * as React from "react";
import { Layers3, X } from "lucide-react";

export interface SessionKeywordBarProps {
  sessionId: string;
  keywords: string[];
}

export function SessionKeywordBar({ sessionId, keywords }: SessionKeywordBarProps) {
  const [removed, setRemoved] = React.useState<Set<string>>(() => new Set());

  React.useEffect(() => setRemoved(new Set()), [sessionId, keywords]);

  const visibleKeywords = keywords.filter((keyword) => !removed.has(keyword));
  if (visibleKeywords.length === 0) return null;

  return (
    <div className="ai-context-bar" aria-label="附加到本轮对话的上下文">
      <span className="ai-context-label">本轮上下文</span>
      <div className="ai-chat-keywords">
        {visibleKeywords.map((keyword) => (
          <button
            key={keyword}
            type="button"
            className="ai-chat-keyword-pill"
            onClick={() => setRemoved((current) => new Set(current).add(keyword))}
            title={`从本轮上下文移除“${keyword}”`}
            aria-label={`从本轮上下文移除：${keyword}`}
          >
            <Layers3 className="ai-context-chip-icon" size={11} />
            <span>{keyword}</span>
            <X className="ai-context-chip-remove" size={11} />
          </button>
        ))}
      </div>
    </div>
  );
}

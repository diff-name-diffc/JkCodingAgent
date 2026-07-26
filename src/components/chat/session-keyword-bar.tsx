import { Layers3 } from "lucide-react";

export interface SessionKeywordBarProps {
  keywords: string[];
}

/**
 * 会话关键词栏：固定在聊天界面顶部，不随消息滚动。
 *
 * 关键词由模型在会话过程中自主总结、更新，用户不可编辑或删除；
 * 点击关键词仅静默复制到剪贴板（无任何提示）。
 */
export function SessionKeywordBar({ keywords }: SessionKeywordBarProps) {
  if (keywords.length === 0) return null;

  return (
    <div className="ai-session-keyword-bar" aria-label="会话关键词">
      <span className="ai-context-label">会话关键词</span>
      <div className="ai-chat-keywords">
        {keywords.map((keyword) => (
          <button
            key={keyword}
            type="button"
            className="ai-chat-keyword-pill"
            onClick={() => void navigator.clipboard.writeText(keyword)}
            aria-label={`复制关键词：${keyword}`}
          >
            <Layers3 className="ai-context-chip-icon" size={11} />
            <span>{keyword}</span>
          </button>
        ))}
      </div>
    </div>
  );
}

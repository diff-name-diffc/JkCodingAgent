import { memo } from "react";
import { Search, ArrowUp, ArrowDown, X } from "lucide-react";
import { dispatcherChatStyles as styles } from "./dispatcherChatStyles";

interface ChatHeaderProps {
  isPlainChat: boolean;
  thinkingEnabled: boolean;
  isLoading: boolean;
  activePlanPath: string | null;
  autoApprove: boolean;
  mcpIndicator: { color: string; label: string };
  hasMessages: boolean;
  searchOpen: boolean;
  searchQuery: string;
  matchCount: number;
  activeIndex: number;
  searchInputRef: React.RefObject<HTMLInputElement | null>;
  onSearchChange: (e: React.ChangeEvent<HTMLInputElement>) => void;
  onSearchKeyDown: (e: React.KeyboardEvent<HTMLInputElement>) => void;
  onFocusSearch: () => void;
  onCloseSearch: () => void;
  onMoveSearchMatch: (direction: 1 | -1) => void;
  onToggleAutoApprove: () => void;
  onOpenMcpStatus: () => void;
  onClearHistory: () => void;
  onOpenSettings: () => void;
  onClosePanel?: () => void;
}

export const ChatHeader = memo(function ChatHeader({
  isPlainChat,
  thinkingEnabled,
  isLoading,
  activePlanPath,
  autoApprove,
  mcpIndicator,
  hasMessages,
  searchOpen,
  searchQuery,
  matchCount,
  activeIndex,
  searchInputRef,
  onSearchChange,
  onSearchKeyDown,
  onFocusSearch,
  onCloseSearch,
  onMoveSearchMatch,
  onToggleAutoApprove,
  onOpenMcpStatus,
  onClearHistory,
  onOpenSettings,
  onClosePanel,
}: ChatHeaderProps) {
  const normalizedQuery = searchQuery.trim();

  return (
    <div style={styles.header}>
      <div style={styles.headerLeft}>
        <span style={styles.headerIcon}>{isPlainChat ? "💬" : "🤖"}</span>
        <span style={styles.headerTitle}>{isPlainChat ? "聊天" : "调度智能体"}</span>
        {!isPlainChat && activePlanPath && <span style={styles.headerPlanBadge}>Plan</span>}
        {thinkingEnabled && <span style={styles.headerThinkingBadge}>Think</span>}
        {isLoading && <span style={styles.thinkingDot} />}
      </div>
      <div style={styles.headerRight}>
        {searchOpen ? (
          <div style={styles.conversationSearchBox}>
            <Search size={13} />
            <input
              ref={searchInputRef}
              style={styles.conversationSearchInput}
              value={searchQuery}
              onChange={onSearchChange}
              onKeyDown={onSearchKeyDown}
              placeholder="搜索会话"
              aria-label="搜索当前会话"
            />
            <span style={styles.conversationSearchCount}>
              {normalizedQuery
                ? matchCount > 0
                  ? `${activeIndex + 1}/${matchCount}`
                  : "0/0"
                : "Ctrl+F"}
            </span>
            <button
              type="button"
              style={styles.conversationSearchNavBtn(matchCount === 0)}
              onClick={() => onMoveSearchMatch(-1)}
              disabled={matchCount === 0}
              title="上一个匹配"
              aria-label="上一个匹配"
            >
              <ArrowUp size={13} />
            </button>
            <button
              type="button"
              style={styles.conversationSearchNavBtn(matchCount === 0)}
              onClick={() => onMoveSearchMatch(1)}
              disabled={matchCount === 0}
              title="下一个匹配"
              aria-label="下一个匹配"
            >
              <ArrowDown size={13} />
            </button>
            <button
              type="button"
              style={styles.conversationSearchNavBtn(false)}
              onClick={onCloseSearch}
              title="关闭搜索"
              aria-label="关闭搜索"
            >
              <X size={13} />
            </button>
          </div>
        ) : (
          <button
            type="button"
            style={styles.headerBtn}
            onClick={onFocusSearch}
            title="搜索当前会话 (Ctrl+F)"
            aria-label="搜索当前会话"
          >
            <Search size={13} />
            搜索
          </button>
        )}
        {!isPlainChat && (
          <>
            <button
              style={{
                ...styles.headerBtn,
                ...(autoApprove ? styles.headerBtnActive : {}),
              }}
              onClick={onToggleAutoApprove}
              title="开启后，调度给 Claude 或 Codex 子任务时不再弹出审查确认"
            >
              免确认 {autoApprove ? "开" : "关"}
            </button>
            <button
              style={styles.headerBtn}
              onClick={onOpenMcpStatus}
              title={`项目级 MCP 状态：${mcpIndicator.label}`}
            >
              <span
                style={{
                  ...styles.headerSignal,
                  background: mcpIndicator.color,
                  boxShadow: `0 0 0 3px ${mcpIndicator.color}22`,
                }}
              />
              MCP
            </button>
          </>
        )}
        {hasMessages && (
          <button style={styles.headerBtn} onClick={onClearHistory}>
            清空
          </button>
        )}
        <button style={styles.headerBtn} onClick={onOpenSettings}>
          ⚙ 设置
        </button>
        {onClosePanel && (
          <button
            style={styles.headerBtn}
            onClick={onClosePanel}
            title="关闭会话面板"
            aria-label="关闭会话面板"
          >
            <X size={14} />
          </button>
        )}
      </div>
    </div>
  );
});

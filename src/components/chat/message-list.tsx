import * as React from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useAutoScroll } from "../../hooks/use-auto-scroll";
import type {
  DispatcherMessage,
  DispatcherToolArtifactRef,
  PythonCodeRunRecord,
} from "../../types";
import type { DispatcherLiveSessionState } from "../dispatcherSessionStore";
import { cn } from "../../lib/cn";
import { isModelNotConfiguredError } from "../../lib/run-error-classify";
import { Button } from "../ui/button";
import { EmptyChatState } from "./empty-chat-state";
import type { ChatEmptyStateContent } from "./chat-empty-content";
import { MessageItem, buildItems, type MessageDisplayItem } from "./message-item";
import {
  OVERSCAN_ROWS,
  ROW_ESTIMATE_PX,
  shouldUseWindowing,
} from "./message-list-metrics";
import { RowUiStateProvider, useRowUiStateStore } from "./row-ui-state";
import { StreamingMessage } from "./streaming-message";
import { ChatScrollAnchor } from "./chat-scroll-anchor";
import { useCopyOnSelect } from "./use-copy-on-select";
import type { ToolActivityItem } from "../dispatcher-chat/tool-activity";

/**
 * Scrollable message list with streaming-aware auto-scroll.
 *
 * Layout contract: the parent must give this component a bounded height
 * (flex-1 + min-h-0 inside <ChatShell />). The list owns its own vertical
 * scroll via the auto-scroll hook.
 *
 * Auto-follow logic (see use-auto-scroll.ts):
 *   - When the user is at the bottom, new streaming content pushes the view.
 *   - When the user scrolls up, follow stops; a floating "最新" button appears.
 *
 * 窗口化（UI-24b-3）：>300 条时启用 @tanstack/react-virtual 动态测量——
 * estimateSize 只作首帧初值，每行挂载后经 measureElement（ResizeObserver）
 * 实测修正，替换 24a 时代「固定 180px 估高 + spacer」的跳读/空白根因
 * （审计 A11）。行卸载丢失的展开态由 RowUiStateProvider 的行级 store 恢复
 * （UI-24b-1）；pinned 跟随仍由 DOM 级 useAutoScroll 承担（滚动容器
 * scrollHeight 含虚拟化总高 div，两套机制无写入冲突）。
 */
export interface MessageListProps {
  sessionId: string | null;
  messages: DispatcherMessage[];
  liveState: DispatcherLiveSessionState | null;
  pythonRunRecords?: Record<string, PythonCodeRunRecord>;
  onRunPython?: (target: {
    messageId: string;
    codeBlockIndex: number;
    code: string;
    codeHash: string;
  }) => void;
  onCopyMessage?: (text: string) => void;
  onRegenerateFromMessage?: (message: DispatcherMessage) => void;
  onEditMessage?: (message: DispatcherMessage) => void;
  onOpenArtifact?: (artifact: DispatcherToolArtifactRef) => void;
  onOpenSubAgent?: (tool: ToolActivityItem) => void;
  onPickPrompt?: (prompt: string) => void;
  /**
   * 发送失败为「模型未配置」类错误时的深链回调（UI-25 遗留 c）。命中分类
   * 且回调存在时，runError 块渲染「配置模型」按钮直达设置对应分类；缺省
   * 则仅展示错误文案（向后兼容其余 MessageList 调用方，如架构助手另有深链）。
   */
  onConfigureModel?: () => void;
  /** 领域化空态文案（UI-15 A06）：不传则用通用聊天欢迎语。 */
  emptyState?: ChatEmptyStateContent;
  className?: string;
}

/** EmptyChatState 的领域化覆盖项（全部可选）。权威定义在 chat-empty-content。 */
export type { ChatEmptyStateContent };

export function MessageList(props: MessageListProps) {
  const rowUiState = useRowUiStateStore();
  return (
    <RowUiStateProvider value={rowUiState}>
      <MessageListInner {...props} />
    </RowUiStateProvider>
  );
}

function MessageListInner({
  sessionId,
  messages,
  liveState,
  pythonRunRecords,
  onRunPython,
  onCopyMessage,
  onRegenerateFromMessage,
  onEditMessage,
  onOpenArtifact,
  onOpenSubAgent,
  onPickPrompt,
  onConfigureModel,
  emptyState,
  className,
}: MessageListProps) {
  // Rebuild display items only when the message array identity changes.
  const items: MessageDisplayItem[] = React.useMemo(() => buildItems(messages), [messages]);

  const isStreaming = Boolean(liveState && (liveState.hasPendingRun || liveState.isLoading));
  const hasLiveContent =
    (liveState?.streamingSegments.length ?? 0) > 0 ||
    (liveState?.liveToolCalls.length ?? 0) > 0 ||
    Boolean(liveState?.liveThinking) ||
    Boolean(liveState?.assistantPlaceholder);

  const { containerRef, pinned, scrollToBottom } = useAutoScroll(sessionId);
  const handleCopyOnSelect = useCopyOnSelect();

  // useAutoScroll 的 containerRef 是回调 ref；本地组合 ref 供 virtualizer 的
  // getScrollElement 读取（挂载后由 measure() effect 触发 virtualizer 感知）。
  const scrollElementRef = React.useRef<HTMLDivElement | null>(null);
  const attachContainerRef = React.useCallback(
    (element: HTMLDivElement | null) => {
      scrollElementRef.current = element;
      containerRef(element);
    },
    [containerRef],
  );

  const useWindowing = shouldUseWindowing(items.length);
  const virtualizer = useVirtualizer({
    count: useWindowing ? items.length : 0,
    getScrollElement: () => scrollElementRef.current,
    estimateSize: () => ROW_ESTIMATE_PX,
    overscan: OVERSCAN_ROWS,
  });

  // 测量缓存按索引键控：items 数组身份变化（会话切换、截断/regenerate、
  // finalize 追加）时清空重测，防止旧行高错位到新内容。流式期间 messages
  // 身份稳定（活内容走 liveState 气泡），不会造成逐 token 清缓存。
  React.useEffect(() => {
    virtualizer.measure();
  }, [items, virtualizer]);

  const isEmpty = items.length === 0 && !hasLiveContent;

  if (isEmpty) {
    return (
      <EmptyChatState
        onPickPrompt={(p) => onPickPrompt?.(p)}
        title={emptyState?.title}
        copy={emptyState?.copy}
        prompts={emptyState?.prompts}
        className={className}
      />
    );
  }

  const renderRow = (item: MessageDisplayItem) => (
    <MessageItem
      key={item.id}
      item={item}
      pythonRunRecords={pythonRunRecords}
      onRunPython={onRunPython}
      onCopyMessage={onCopyMessage}
      onRegenerateFromMessage={onRegenerateFromMessage}
      onEditMessage={item.kind === "user" ? onEditMessage : undefined}
      onOpenArtifact={onOpenArtifact}
      onOpenSubAgent={onOpenSubAgent}
    />
  );

  // 行间距语义对齐非窗口化路径的 gap-6（24px）：窗口化下 gap 不参与绝对
  // 定位，改为烘焙进行高——每行 pb-6，由 measureElement 一并实测。
  const virtualRows = useWindowing
    ? virtualizer.getVirtualItems().map((virtualRow) => {
        const item = items[virtualRow.index];
        return (
          <div
            key={item.id}
            ref={virtualizer.measureElement}
            data-index={virtualRow.index}
            className="absolute inset-x-0 top-0 pb-6"
            style={{ transform: `translateY(${virtualRow.start}px)` }}
          >
            {renderRow(item)}
          </div>
        );
      })
    : null;

  return (
    <div className={cn("relative min-h-0 flex-1", className)}>
      <div
        ref={attachContainerRef}
        className="chat-scroll h-full overflow-y-auto overflow-x-hidden"
        role="log"
        aria-live="polite"
        aria-busy={isStreaming}
        onMouseUp={handleCopyOnSelect}
      >
        {/* 外层 column 是 useAutoScroll ResizeObserver 的观察目标
            （firstElementChild）：窗口化时行高实测修正与流式气泡增长都
            体现为它的高度变化，pinned 跟随据此触发。 */}
        <div
          className={cn(
            "chat-prose flex flex-col pt-6",
            !useWindowing && "gap-6",
            hasLiveContent || liveState?.runError ? "" : "pb-6",
          )}
        >
          {useWindowing ? (
            <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
              {virtualRows}
            </div>
          ) : (
            items.map(renderRow)
          )}

          {/* Trailing live streaming bubble（窗口化时位于总高 div 之后的
              常规流，行 pb-6 已提供与末行的 24px 间距） */}
          {hasLiveContent && liveState && (
            <StreamingMessage
              segments={liveState.streamingSegments}
              tools={liveState.liveToolCalls}
              thinking={liveState.liveThinking}
              placeholder={liveState.assistantPlaceholder}
              isStreaming={isStreaming}
              showAvatar={items[items.length - 1]?.kind !== "assistant"}
              onOpenArtifact={onOpenArtifact}
              onOpenSubAgent={onOpenSubAgent}
            />
          )}

          {liveState?.runError && (
            <div className="rounded-lg border border-destructive/40 bg-destructive/10 px-4 py-2.5 text-sm">
              <div className="flex flex-wrap items-center gap-2">
                <span className="text-destructive">错误：{liveState.runError}</span>
                {onConfigureModel && isModelNotConfiguredError(liveState.runError) && (
                  <Button variant="outline" size="sm" onClick={onConfigureModel}>
                    配置模型
                  </Button>
                )}
              </div>
            </div>
          )}
        </div>
      </div>

      <ChatScrollAnchor
        showJumpButton={!pinned && items.length > 0}
        onJumpToLatest={() => scrollToBottom({ behavior: "smooth" })}
      />
    </div>
  );
}

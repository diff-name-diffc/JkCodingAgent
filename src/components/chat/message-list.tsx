import * as React from "react";
import { useAutoScroll } from "../../hooks/use-auto-scroll";
import type {
  DispatcherMessage,
  DispatcherToolArtifactRef,
  PythonCodeRunRecord,
} from "../../types";
import type { DispatcherLiveSessionState } from "../dispatcherSessionStore";
import { cn } from "../../lib/cn";
import { EmptyChatState } from "./empty-chat-state";
import type { ChatEmptyStateContent } from "./chat-empty-content";
import { MessageItem, buildItems, type MessageDisplayItem } from "./message-item";
import {
  computeWindowRange,
  windowSpacerHeights,
} from "./message-list-metrics";
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
  /** 领域化空态文案（UI-15 A06）：不传则用通用聊天欢迎语。 */
  emptyState?: ChatEmptyStateContent;
  className?: string;
}

/** EmptyChatState 的领域化覆盖项（全部可选）。权威定义在 chat-empty-content。 */
export type { ChatEmptyStateContent };

export function MessageList({
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

  // 滚动度量（UI-24a-3）：单一 state + rAF 合帧——旧实现每个 scroll 事件
  // setState×2 无节流；viewportHeight 只在 scroll 事件更新，容器 resize
  // （切布局/分栏）不产生 scroll 事件，留下过期值。ResizeObserver 补上
  // resize 通道；保活隐藏/折叠容器的 0 高度回调忽略，防窗口化参数被打成 0。
  // useAutoScroll 的 containerRef 是回调 ref，本地组合 ref+state 跟踪元素。
  const scrollElementRef = React.useRef<HTMLDivElement | null>(null);
  const [scrollElement, setScrollElement] = React.useState<HTMLDivElement | null>(null);
  const attachContainerRef = React.useCallback(
    (element: HTMLDivElement | null) => {
      scrollElementRef.current = element;
      setScrollElement(element);
      containerRef(element);
    },
    [containerRef],
  );
  const [scrollMetrics, setScrollMetrics] = React.useState({ scrollTop: 0, viewportHeight: 720 });
  const scrollRafRef = React.useRef<number | null>(null);
  const readScrollMetrics = React.useCallback(() => {
    scrollRafRef.current = null;
    const el = scrollElementRef.current;
    if (!el) return;
    const viewportHeight = el.clientHeight;
    if (viewportHeight === 0) return;
    setScrollMetrics((prev) =>
      prev.scrollTop === el.scrollTop && prev.viewportHeight === viewportHeight
        ? prev
        : { scrollTop: el.scrollTop, viewportHeight },
    );
  }, []);
  const scheduleScrollRead = React.useCallback(() => {
    if (scrollRafRef.current !== null) return;
    scrollRafRef.current = window.requestAnimationFrame(readScrollMetrics);
  }, [readScrollMetrics]);
  React.useEffect(
    () => () => {
      if (scrollRafRef.current !== null) window.cancelAnimationFrame(scrollRafRef.current);
    },
    [],
  );

  // 窗口化本体（估高/阈值）属 UI-24b 改造范围，此处语义与旧实现一致：
  // >300 条才开窗，固定 180px 估高 ± 8 行 overscan。
  const { useWindowing, startIndex, endIndex } = computeWindowRange({
    itemCount: items.length,
    scrollTop: scrollMetrics.scrollTop,
    viewportHeight: scrollMetrics.viewportHeight,
  });
  // 仅开窗后才需要滚动度量：未开窗时 scroll/resize 不触发任何 setState。
  const windowingRef = React.useRef(useWindowing);
  windowingRef.current = useWindowing;
  const handleScroll = React.useCallback(() => {
    if (windowingRef.current) scheduleScrollRead();
  }, [scheduleScrollRead]);

  // 容器尺寸观测：切布局/分栏/窗口缩放后 viewportHeight 保持新鲜；
  // 挂载时实测初值（720 仅首帧 fallback）。空态分支不渲染滚动容器，
  // scrollElement 为 null 时自动跳过。
  React.useLayoutEffect(() => {
    if (!scrollElement) return;
    readScrollMetrics();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      if (windowingRef.current) scheduleScrollRead();
    });
    observer.observe(scrollElement);
    return () => observer.disconnect();
  }, [scrollElement, readScrollMetrics, scheduleScrollRead]);

  const isEmpty = items.length === 0 && !hasLiveContent;
  const visibleItems = useWindowing ? items.slice(startIndex, endIndex) : items;
  const spacers = windowSpacerHeights(
    { useWindowing, startIndex, endIndex },
    items.length,
  );

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

  return (
    <div className={cn("relative min-h-0 flex-1", className)}>
      <div
        ref={attachContainerRef}
        className="chat-scroll h-full overflow-y-auto overflow-x-hidden"
        role="log"
        aria-live="polite"
        aria-busy={isStreaming}
        onMouseUp={handleCopyOnSelect}
        onScroll={handleScroll}
      >
        <div className="chat-prose flex flex-col gap-6 py-6">
          {useWindowing && <div style={{ height: spacers.top }} />}
          {visibleItems.map((item) => (
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
          ))}
          {useWindowing && <div style={{ height: spacers.bottom }} />}

          {/* Trailing live streaming bubble */}
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
            <div className="rounded-lg border border-destructive/40 bg-destructive/10 px-4 py-2.5 text-sm text-destructive">
              错误：{liveState.runError}
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

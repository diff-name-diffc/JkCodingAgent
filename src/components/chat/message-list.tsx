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
import { MessageItem, buildItems, type MessageDisplayItem } from "./message-item";
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
  className?: string;
}

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
  const [scrollTop, setScrollTop] = React.useState(0);
  const [viewportHeight, setViewportHeight] = React.useState(720);
  const rowEstimate = 180;
  const overscan = 8;
  const useWindowing = items.length > 300;
  const startIndex = useWindowing ? Math.max(0, Math.floor(scrollTop / rowEstimate) - overscan) : 0;
  const endIndex = useWindowing
    ? Math.min(items.length, Math.ceil((scrollTop + viewportHeight) / rowEstimate) + overscan)
    : items.length;
  const visibleItems = useWindowing ? items.slice(startIndex, endIndex) : items;

  const isEmpty = items.length === 0 && !hasLiveContent;

  if (isEmpty) {
    return <EmptyChatState onPickPrompt={(p) => onPickPrompt?.(p)} />;
  }

  return (
    <div className={cn("relative min-h-0 flex-1", className)}>
      <div
        ref={containerRef}
        className="chat-scroll h-full overflow-y-auto overflow-x-hidden"
        role="log"
        aria-live="polite"
        aria-busy={isStreaming}
        onMouseUp={handleCopyOnSelect}
        onScroll={(event) => {
          if (useWindowing) {
            setScrollTop(event.currentTarget.scrollTop);
            setViewportHeight(event.currentTarget.clientHeight);
          }
        }}
      >
        <div className="chat-prose flex flex-col gap-6 py-6">
          {useWindowing && <div style={{ height: startIndex * rowEstimate }} />}
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
          {useWindowing && (
            <div style={{ height: Math.max(0, (items.length - endIndex) * rowEstimate) }} />
          )}

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

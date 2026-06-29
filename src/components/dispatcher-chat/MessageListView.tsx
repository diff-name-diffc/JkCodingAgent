import { memo, type RefObject } from "react";
import type {
  ChecklistPlanState,
  DispatcherMessageUsageStats,
  PlanInteraction,
  PythonCodeRunRecord,
  PythonCodeRunTarget,
} from "../../types";
import type { AssistantThinkingBlock, AssistantTurnSegment } from "../dispatcherChatView";
import type { ToolActivityItem } from "../ToolActivityBubble";
import { UserMessageBubble, AssistantTurnBubble } from "./MessageBubbles";
import { InteractionDrawer } from "./InteractionDrawer";
import { useSubAgentProgressMessages } from "../subAgentEventStore";
import { dispatcherChatStyles as styles } from "./dispatcherChatStyles";

interface MessageListProps {
  displayItems: ReturnType<typeof import("../dispatcherChatView").buildDispatcherDisplayItems>;
  streamingSegments: AssistantTurnSegment[];
  liveThinking: AssistantThinkingBlock | null;
  showThinking: boolean;
  liveToolCalls: ToolActivityItem[];
  assistantPlaceholder: string | null;
  liveUsageStats: DispatcherMessageUsageStats | null;
  isStreaming: boolean;
  isEmpty: boolean;
  isPlainChat: boolean;
  runError: string | null;
  sessionId: string;
  checklist: ChecklistPlanState | null;
  planInteraction: PlanInteraction | null;
  implementingPlan: boolean;
  messageListRef: RefObject<HTMLDivElement | null>;
  onScroll: (event: React.UIEvent<HTMLDivElement>) => void;
  onAnswerPlanQuestion: (answer: string) => void;
  onImplementPlan: (interaction: Extract<PlanInteraction, { kind: "ready" }>) => void;
  onImplementPlanWithClearedContext: (
    interaction: Extract<PlanInteraction, { kind: "ready" }>,
  ) => void;
  onStayInPlanMode: () => void;
  onRunPython?: (target: PythonCodeRunTarget) => void;
  pythonRunRecords?: Record<string, PythonCodeRunRecord>;
}

export const MessageList = memo(function MessageList({
  displayItems,
  streamingSegments,
  liveThinking,
  showThinking,
  liveToolCalls,
  assistantPlaceholder,
  liveUsageStats,
  isStreaming,
  isEmpty,
  isPlainChat,
  runError,
  sessionId,
  checklist,
  planInteraction,
  implementingPlan,
  messageListRef,
  onScroll,
  onAnswerPlanQuestion,
  onImplementPlan,
  onImplementPlanWithClearedContext,
  onStayInPlanMode,
  onRunPython,
  pythonRunRecords,
}: MessageListProps) {
  const subAgentProgressMessages = useSubAgentProgressMessages(sessionId);
  const subAgentProgressSegments: AssistantTurnSegment[] = subAgentProgressMessages.map(
    (message) => ({
      kind: "assistant-text",
      text: message.text,
    }),
  );
  const hasLiveSegments = streamingSegments.some((segment) => segment.text.trim());
  const hasAssistantPlaceholder = Boolean(assistantPlaceholder?.trim());
  const hasSubAgentProgress = subAgentProgressSegments.length > 0;
  const shouldAttachProgressToLiveTurn =
    hasSubAgentProgress &&
    (isStreaming ||
      hasLiveSegments ||
      liveThinking ||
      liveToolCalls.length > 0 ||
      hasAssistantPlaceholder);
  const lastAssistantItemId = [...displayItems]
    .reverse()
    .find((item) => item.kind === "assistant")?.id;

  return (
    <div
      ref={messageListRef}
      className="dispatcher-message-list"
      style={styles.messageList}
      onScroll={onScroll}
    >
      {runError && <div style={styles.runErrorBanner}>{runError}</div>}
      {isEmpty && !isPlainChat && (
        <InteractionDrawer
          checklist={checklist}
          planInteraction={planInteraction}
          implementingPlan={implementingPlan}
          onAnswerPlanQuestion={onAnswerPlanQuestion}
          onImplementPlan={onImplementPlan}
          onImplementPlanWithClearedContext={onImplementPlanWithClearedContext}
          onStayInPlanMode={onStayInPlanMode}
        />
      )}
      {displayItems.map((item) => {
        if (item.kind === "user") {
          return (
            <UserMessageBubble
              key={item.id}
              message={item.message}
            />
          );
        }
        return (
          <AssistantTurnBubble
            key={item.id}
            segments={
              !shouldAttachProgressToLiveTurn && item.id === lastAssistantItemId
                ? [...item.turn.segments, ...subAgentProgressSegments]
                : item.turn.segments
            }
            tools={item.turn.tools}
            workspaceId={sessionId}
            usageStats={item.turn.usageStats}
            thinking={showThinking ? item.turn.thinking : null}
            onRunPython={onRunPython}
            pythonRunRecords={pythonRunRecords}
          />
        );
      })}
      {(hasLiveSegments ||
        liveThinking ||
        liveToolCalls.length > 0 ||
        hasAssistantPlaceholder ||
        liveUsageStats ||
        (hasSubAgentProgress && (shouldAttachProgressToLiveTurn || !lastAssistantItemId))) && (
        <AssistantTurnBubble
          segments={
            shouldAttachProgressToLiveTurn || !lastAssistantItemId
              ? [...streamingSegments, ...subAgentProgressSegments]
              : streamingSegments
          }
          tools={liveToolCalls}
          workspaceId={sessionId}
          usageStats={liveUsageStats}
          thinking={showThinking ? liveThinking : null}
          placeholderText={assistantPlaceholder}
          streaming={isStreaming}
        />
      )}
    </div>
  );
});

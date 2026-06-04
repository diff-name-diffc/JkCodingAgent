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
import { dispatcherChatStyles as styles } from "./dispatcherChatStyles";

interface MessageListProps {
  displayItems: ReturnType<typeof import("../dispatcherChatView").buildDispatcherDisplayItems>;
  streamingSegments: AssistantTurnSegment[];
  liveThinking: AssistantThinkingBlock | null;
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
  onImplementPlanWithClearedContext: (interaction: Extract<PlanInteraction, { kind: "ready" }>) => void;
  onStayInPlanMode: () => void;
  onRunPython?: (target: PythonCodeRunTarget) => void;
  pythonRunRecords?: Record<string, PythonCodeRunRecord>;
}

export const MessageList = memo(function MessageList({
  displayItems,
  streamingSegments,
  liveThinking,
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
  const hasLiveSegments = streamingSegments.some((segment) => segment.text.trim());
  const hasAssistantPlaceholder = Boolean(assistantPlaceholder?.trim());

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
      {displayItems.map((item) =>
        item.kind === "user" ? (
          <UserMessageBubble key={item.id} message={item.message} onRunPython={onRunPython} pythonRunRecords={pythonRunRecords} />
        ) : (
          <AssistantTurnBubble
            key={item.id}
            segments={item.turn.segments}
            tools={item.turn.tools}
            workspaceId={sessionId}
            usageStats={item.turn.usageStats}
            thinking={item.turn.thinking}
            onRunPython={onRunPython}
            pythonRunRecords={pythonRunRecords}
          />
        ),
      )}
      {(hasLiveSegments ||
        liveThinking ||
        liveToolCalls.length > 0 ||
        hasAssistantPlaceholder ||
        liveUsageStats) && (
        <AssistantTurnBubble
          segments={streamingSegments}
          tools={liveToolCalls}
          workspaceId={sessionId}
          usageStats={liveUsageStats}
          thinking={liveThinking}
          placeholderText={assistantPlaceholder}
          streaming={isStreaming}
        />
      )}
    </div>
  );
});

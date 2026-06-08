import { memo, useMemo, type RefObject } from "react";
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
import { SubAgentExecutionCard } from "../SubAgentExecutionView";
import {
  extractAgentIdsFromToolInput,
  useSubAgentSessions,
} from "../subAgentEventStore";
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
  onImplementPlanWithClearedContext: (interaction: Extract<PlanInteraction, { kind: "ready" }>) => void;
  onStayInPlanMode: () => void;
  onRunPython?: (target: PythonCodeRunTarget) => void;
  pythonRunRecords?: Record<string, PythonCodeRunRecord>;
}

function extractSubAgentIdsFromTools(tools: ToolActivityItem[]): string[] {
  const ids: string[] = [];
  for (const tool of tools) {
    if (tool.name !== "call_sub_agent") continue;
    const agentId = extractAgentIdsFromToolInput(tool.input);
    if (agentId) ids.push(agentId);
  }
  return ids;
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
  const subAgentSessions = useSubAgentSessions(sessionId);

  const hasLiveSegments = streamingSegments.some((segment) => segment.text.trim());
  const hasAssistantPlaceholder = Boolean(assistantPlaceholder?.trim());
  const liveSubAgentIds = useMemo(
    () => extractSubAgentIdsFromTools(liveToolCalls),
    [liveToolCalls],
  );

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
              onRunPython={onRunPython}
              pythonRunRecords={pythonRunRecords}
            />
          );
        }
        const turnSubAgentIds = extractSubAgentIdsFromTools(item.turn.tools);
        const turnCards = turnSubAgentIds
          .map((agentId) => subAgentSessions[agentId])
          .filter(Boolean);
        return (
          <div key={item.id}>
            {turnCards.length > 0 && (
              <div style={{ display: "flex", flexDirection: "column", gap: 8, margin: "8px 0" }}>
                {turnCards.map((session) => (
                  <SubAgentExecutionCard key={session.agentId} session={session} />
                ))}
              </div>
            )}
            <AssistantTurnBubble
              segments={item.turn.segments}
              tools={item.turn.tools}
              workspaceId={sessionId}
              usageStats={item.turn.usageStats}
              thinking={showThinking ? item.turn.thinking : null}
              onRunPython={onRunPython}
              pythonRunRecords={pythonRunRecords}
            />
          </div>
        );
      })}
      {(hasLiveSegments ||
        liveThinking ||
        liveToolCalls.length > 0 ||
        hasAssistantPlaceholder ||
        liveUsageStats) && (
        <div>
          {liveSubAgentIds.length > 0 && (
            <div style={{ display: "flex", flexDirection: "column", gap: 8, margin: "8px 0" }}>
              {liveSubAgentIds
                .map((agentId) => subAgentSessions[agentId])
                .filter(Boolean)
                .map((session) => (
                  <SubAgentExecutionCard key={session.agentId} session={session} />
                ))}
            </div>
          )}
          <AssistantTurnBubble
            segments={streamingSegments}
            tools={liveToolCalls}
            workspaceId={sessionId}
            usageStats={liveUsageStats}
            thinking={showThinking ? liveThinking : null}
            placeholderText={assistantPlaceholder}
            streaming={isStreaming}
          />
        </div>
      )}
    </div>
  );
});

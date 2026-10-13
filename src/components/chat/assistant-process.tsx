import { memo, useMemo } from "react";
import { Activity, ChevronDown, Loader2, MessageSquare } from "lucide-react";
import type {
  AssistantThinkingBlock,
  AssistantTurnSegment,
} from "../dispatcher-chat/assistant-segments";
import { summarizeToolActivity, type ToolActivityItem } from "../dispatcher-chat/tool-activity";
import { cn } from "../../lib/cn";
import { Button } from "../ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "../ui/collapsible";
import { MarkdownRenderer, type MarkdownRendererProps } from "./markdown-renderer";
import { ReasoningBlock } from "./reasoning-block";
import { ToolCallList } from "./tool-call-card";
import type { ToolActivityActions } from "./tool-activity-row";
import { toolWorkflowPlanId } from "./tool-activity-presentation";
import { usePersistedToggle, useRowUiStateContext } from "./row-ui-state";

type ContentProps = Pick<MarkdownRendererProps, "messageId" | "pythonRunRecords" | "onRunPython">;

interface AssistantProcessProps extends ContentProps, ToolActivityActions {
  rowId?: string;
  thinking?: AssistantThinkingBlock | null;
  tools: ToolActivityItem[];
  progressSegments: AssistantTurnSegment[];
  summaries: AssistantTurnSegment[];
  isStreaming: boolean;
  isThinking: boolean;
  hasReply: boolean;
  statusText?: string;
}

/** 执行记录与回复分区；所有折叠键沿用同一轮次的行级 store。 */
export const AssistantProcess = memo(function AssistantProcess({
  rowId,
  thinking,
  tools,
  progressSegments,
  summaries,
  isStreaming,
  isThinking,
  hasReply,
  statusText,
  messageId,
  pythonRunRecords,
  onRunPython,
  ...toolActions
}: AssistantProcessProps) {
  const store = useRowUiStateContext();
  const summary = useMemo(() => summarizeToolActivity(tools), [tools]);
  // 收起整个过程仍保留异常、未结束工具和工作流操作入口。
  const attentionTools = useMemo(
    () =>
      tools.filter((tool) => tool.planned || tool.status !== "success" || toolWorkflowPlanId(tool)),
    [tools],
  );
  const hasExpandedChild = Boolean(
    rowId !== undefined &&
    (store?.get(`reasoning:${rowId}`) ||
      store?.get(`tools:${rowId}`) ||
      store?.get(`progress:${rowId}`) ||
      tools.some((tool) => store?.get(`card:${tool.id}`))),
  );
  const [open, setOpen] = usePersistedToggle(
    rowId === undefined ? undefined : `process:${rowId}`,
    isStreaming || !hasReply || attentionTools.length > 0 || hasExpandedChild,
  );

  return (
    <Collapsible open={open} onOpenChange={setOpen} className="ai-assistant-process">
      <CollapsibleTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          className="ai-process-trigger h-auto min-h-10 w-full justify-start whitespace-normal rounded-none px-3 py-2 text-left"
        >
          <Activity aria-hidden className="text-muted-foreground" />
          <span className="flex min-w-0 flex-1 flex-wrap items-center gap-x-3 gap-y-1">
            <span className="text-[13px] font-medium">执行过程</span>
            <span className="text-xs font-normal text-muted-foreground">
              {tools.length > 0 ? `${tools.length} 项工具` : "思考与进度"}
            </span>
            <span className="ml-auto flex flex-wrap justify-end gap-x-2 gap-y-1 text-xs font-normal">
              {summary.failed > 0 && (
                <span className="text-destructive">{summary.failed} 项失败</span>
              )}
              {summary.planned > 0 && <span>{summary.planned} 项等待</span>}
              {(isStreaming || summary.running > 0) && <span>进行中</span>}
            </span>
          </span>
          <ChevronDown
            aria-hidden
            className={cn(
              "text-muted-foreground transition-transform duration-fast motion-reduce:transition-none",
              open && "rotate-180",
            )}
          />
        </Button>
      </CollapsibleTrigger>
      {statusText && (
        <div className="ai-agent-status px-3 pb-2" role="status" aria-live="polite">
          <Loader2
            aria-hidden
            className="h-3.5 w-3.5 shrink-0 animate-spin motion-reduce:animate-none"
          />
          <span>{statusText}</span>
        </div>
      )}
      <CollapsibleContent className="ai-process-content">
        {thinking?.text.trim() && (
          <ReasoningBlock
            className="ai-process-section"
            text={thinking.text}
            elapsedMs={thinking.elapsedMs}
            isStreaming={isThinking}
            autoOpen={isStreaming}
            persistKey={rowId === undefined ? undefined : `reasoning:${rowId}`}
          />
        )}
        {tools.length > 0 && (
          <ToolCallList
            items={tools}
            rowId={rowId}
            className="ai-process-section ai-process-tools"
            {...toolActions}
          />
        )}
        {progressSegments.length > 0 && (
          <ProgressNotes
            segments={progressSegments}
            rowId={rowId}
            isStreaming={isStreaming}
            messageId={messageId}
            pythonRunRecords={pythonRunRecords}
            onRunPython={onRunPython}
          />
        )}
        {summaries.map((segment, index) => (
          <section
            className="ai-process-section p-3"
            aria-label="工具摘要"
            key={segment.toolCallId ?? segment.messageId ?? index}
          >
            <div className="mb-2 text-xs text-muted-foreground">工具摘要</div>
            <MarkdownRenderer
              content={segment.text}
              className="ai-process-markdown text-[13px] leading-[22px] text-[var(--text-secondary)]"
              messageId={segment.messageId ?? messageId}
              pythonRunRecords={pythonRunRecords}
              onRunPython={onRunPython}
            />
          </section>
        ))}
      </CollapsibleContent>
      {!open && attentionTools.length > 0 && (
        <ToolCallList
          items={attentionTools}
          rowId={rowId}
          className="ai-process-section ai-process-tools"
          {...toolActions}
        />
      )}
    </Collapsible>
  );
});

function ProgressNotes({
  segments,
  rowId,
  isStreaming,
  messageId,
  pythonRunRecords,
  onRunPython,
}: ContentProps & { segments: AssistantTurnSegment[]; rowId?: string; isStreaming: boolean }) {
  const [open, setOpen] = usePersistedToggle(
    rowId === undefined ? undefined : `progress:${rowId}`,
    isStreaming,
  );
  return (
    <Collapsible open={open} onOpenChange={setOpen} className="ai-process-section">
      <CollapsibleTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          className="h-auto min-h-9 w-full justify-start rounded-none px-3 py-2 text-xs text-[var(--text-secondary)]"
        >
          <MessageSquare aria-hidden />
          <span>进度说明</span>
          <span className="ml-auto text-muted-foreground">{segments.length} 条</span>
          <ChevronDown
            aria-hidden
            className={cn(
              "transition-transform duration-fast motion-reduce:transition-none",
              open && "rotate-180",
            )}
          />
        </Button>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="space-y-3 px-3 pb-3">
          {segments.map((segment, index) => (
            <MarkdownRenderer
              key={segment.messageId ?? `progress-${index}`}
              content={segment.text}
              className="ai-process-markdown text-[13px] leading-[22px] text-[var(--text-secondary)]"
              messageId={segment.messageId ?? messageId}
              pythonRunRecords={pythonRunRecords}
              onRunPython={onRunPython}
            />
          ))}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}

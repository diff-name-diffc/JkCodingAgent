import { memo } from "react";
import { Loader2 } from "lucide-react";
import type {
  DispatcherMessageUsageStats,
  DispatcherToolArtifactRef,
  ModelCategory,
} from "../../types";
import type {
  AssistantThinkingBlock,
  AssistantTurnSegment,
} from "../dispatcher-chat/assistant-segments";
import type { ToolActivityItem } from "../dispatcher-chat/tool-activity";
import { cn } from "../../lib/cn";
import { ChatAvatar } from "./chat-avatar";
import { MarkdownRenderer } from "./markdown-renderer";
import { MessageActions } from "./message-actions";
import { ReasoningBlock } from "./reasoning-block";
import { ToolCallList } from "./tool-call-card";
import { formatTokenCountK } from "../dispatcher-chat/dispatcherChatUtils";

/** 同一轮次从流式到完成共用此组件；阶段说明保留原位，不因后续输出而消失。 */
export interface AssistantMessageProps {
  segments: AssistantTurnSegment[];
  tools?: ToolActivityItem[];
  thinking?: AssistantThinkingBlock | null;
  usageStats?: DispatcherMessageUsageStats;
  /** Message id used to anchor markdown + python run records. */
  messageId?: string;
  /** UI-24b-1：消息行稳定 id——窗口化行卸载后组/思考块展开态经行级 store 恢复。 */
  rowId?: string;
  /** 连续 AI 消息分组中仅第一条显示头像（锚点位置保留，仅隐藏）。 */
  showAvatar?: boolean;
  isStreaming?: boolean;
  isThinking?: boolean;
  placeholder?: string | null;
  pythonRunRecords?: Record<string, import("../../types").PythonCodeRunRecord>;
  onRunPython?: (target: {
    messageId: string;
    codeBlockIndex: number;
    code: string;
    codeHash: string;
  }) => void;
  onCopy?: (text: string) => void;
  onRegenerate?: () => void;
  onOpenArtifact?: (artifact: DispatcherToolArtifactRef) => void;
  onOpenSubAgent?: (tool: ToolActivityItem) => void;
  /** UI-25 第四批遗留：工具级「模型未配置」错误深链（透传至 ToolCallList）。 */
  onConfigureModel?: (category?: ModelCategory) => void;
  className?: string;
}

export const AssistantMessage = memo(function AssistantMessage({
  segments,
  tools,
  thinking,
  usageStats,
  messageId,
  rowId,
  showAvatar = true,
  isStreaming = false,
  isThinking = false,
  placeholder,
  pythonRunRecords,
  onRunPython,
  onCopy,
  onRegenerate,
  onOpenArtifact,
  onOpenSubAgent,
  onConfigureModel,
  className,
}: AssistantMessageProps) {
  const textSegments = segments.filter(
    (segment) => segment.kind === "assistant-text" && segment.text.trim(),
  );
  const toolIds = new Set(tools?.map((tool) => tool.id));
  // 工具卡已经持有结果时，不再把同一份压缩输出冒充为助手答复。
  const standaloneSummaries = segments.filter(
    (segment) =>
      segment.kind === "tool-summary" &&
      segment.text.trim() &&
      (!segment.toolCallId || !toolIds.has(segment.toolCallId)),
  );
  const showReasoning = Boolean(thinking?.text?.trim());
  const runningTools = tools?.filter((tool) => tool.status === "running") ?? [];
  const hasActiveText = textSegments.some((segment) => !segment.superseded);
  const isWriting = isStreaming && hasActiveText && !placeholder;
  const statusText =
    placeholder ||
    (runningTools.length > 0
      ? `正在处理工具活动 · ${runningTools.length} 项待完成`
      : isThinking
        ? "正在思考"
        : "正在整理回复");

  const handleCopy = () => {
    const answer = textSegments.filter((segment) => !segment.superseded);
    const text = (answer.length ? answer : textSegments).map((s) => s.text).join("\n\n");
    if (onCopy) onCopy(text);
    else void navigator.clipboard.writeText(text);
  };

  return (
    <div className={cn("ai-assistant-message group relative", className)}>
      <ChatAvatar
        role="assistant"
        active={isStreaming}
        hidden={!showAvatar}
        className="absolute left-6 top-0.5"
      />

      <div className="min-w-0 pl-[60px]">
        {showReasoning && (
          <ReasoningBlock
            className="mb-2"
            text={thinking?.text ?? ""}
            elapsedMs={thinking?.elapsedMs ?? 0}
            isStreaming={isThinking}
            autoOpen={isStreaming}
            persistKey={rowId === undefined ? undefined : `reasoning:${rowId}`}
          />
        )}

        {/* Tool calls */}
        {tools && tools.length > 0 && (
          <ToolCallList
            items={tools}
            className="mb-2"
            rowId={rowId}
            onOpenArtifact={onOpenArtifact}
            onOpenSubAgent={onOpenSubAgent}
            onConfigureModel={onConfigureModel}
          />
        )}

        {standaloneSummaries.map((segment, index) => (
          <section
            key={segment.toolCallId ?? segment.messageId ?? index}
            aria-label="工具摘要"
            className="mb-3 rounded-md border border-border/60 bg-muted/30 p-3"
          >
            <div className="mb-1 text-xs text-muted-foreground">工具摘要</div>
            <MarkdownRenderer
              content={segment.text}
              messageId={segment.messageId ?? messageId}
              pythonRunRecords={pythonRunRecords}
              onRunPython={isStreaming ? undefined : onRunPython}
            />
          </section>
        ))}

        <div className="space-y-4">
          {textSegments.map((segment, index) => (
            <MarkdownRenderer
              key={segment.messageId ?? `text-${index}`}
              content={segment.text}
              className={cn(segment.superseded && "text-[var(--text-secondary)]")}
              streaming={isWriting && !segment.superseded && index === textSegments.length - 1}
              messageId={segment.messageId ?? messageId}
              onRunPython={isStreaming ? undefined : onRunPython}
              pythonRunRecords={pythonRunRecords}
            />
          ))}
        </div>

        {isStreaming && !isWriting && (
          <div className="ai-agent-status" role="status" aria-live="polite">
            <Loader2
              aria-hidden
              className="h-3.5 w-3.5 shrink-0 animate-spin motion-reduce:animate-none"
            />
            <span>{statusText}</span>
          </div>
        )}

        {!isStreaming && (
          <MessageActions
            tokenLabel={
              usageStats ? `${formatTokenCountK(usageStats.totalTokens)} tokens` : undefined
            }
            onCopy={handleCopy}
            onRegenerate={onRegenerate}
          />
        )}
      </div>
    </div>
  );
});

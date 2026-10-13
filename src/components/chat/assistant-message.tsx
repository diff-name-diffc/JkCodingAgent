import { memo } from "react";
import { Loader2, MessageCircle } from "lucide-react";
import type {
  DispatcherMessageUsageStats,
  DispatcherToolArtifactRef,
  ModelCategory,
} from "../../types";
import type {
  AssistantThinkingBlock,
  AssistantTurnSegment,
} from "../dispatcher-chat/assistant-segments";
import { partitionAssistantSegments } from "../dispatcher-chat/assistant-segments";
import type { ToolActivityItem } from "../dispatcher-chat/tool-activity";
import { cn } from "../../lib/cn";
import { ChatAvatar } from "./chat-avatar";
import { MarkdownRenderer } from "./markdown-renderer";
import { MessageActions } from "./message-actions";
import { AssistantProcess } from "./assistant-process";
import { formatTokenCountK } from "../dispatcher-chat/dispatcherChatUtils";

/** 同一轮次从流式到完成共用此组件；执行过程与面向用户的回复各自成区。 */
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

const EMPTY_TOOLS: ToolActivityItem[] = [];

export const AssistantMessage = memo(function AssistantMessage({
  segments,
  tools = EMPTY_TOOLS,
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
  const { progressSegments, answerSegments } = partitionAssistantSegments(segments);
  const toolIds = new Set(tools.map((tool) => tool.id));
  // 工具卡已经持有结果时，不再把同一份压缩输出冒充为助手答复。
  const standaloneSummaries = segments.filter(
    (segment) =>
      segment.kind === "tool-summary" &&
      segment.text.trim() &&
      (!segment.toolCallId || !toolIds.has(segment.toolCallId)),
  );
  const hasProcess = Boolean(
    thinking?.text.trim() || tools.length || progressSegments.length || standaloneSummaries.length,
  );
  const runningTools = tools.filter((tool) => tool.status === "running");
  const isWriting = isStreaming && answerSegments.length > 0 && !placeholder;
  const statusText =
    placeholder ||
    (runningTools.length > 0
      ? `正在处理工具活动 · ${runningTools.length} 项待完成`
      : isThinking
        ? "正在思考"
        : "正在整理回复");

  const handleCopy = () => {
    const text = (answerSegments.length ? answerSegments : progressSegments)
      .map((segment) => segment.text)
      .join("\n\n");
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
        {hasProcess && (
          <AssistantProcess
            rowId={rowId}
            thinking={thinking}
            tools={tools}
            progressSegments={progressSegments}
            summaries={standaloneSummaries}
            isStreaming={isStreaming}
            isThinking={isThinking}
            hasReply={answerSegments.length > 0}
            statusText={isStreaming && !isWriting ? statusText : undefined}
            messageId={messageId}
            pythonRunRecords={pythonRunRecords}
            onRunPython={isStreaming ? undefined : onRunPython}
            onOpenArtifact={onOpenArtifact}
            onOpenSubAgent={onOpenSubAgent}
            onConfigureModel={onConfigureModel}
          />
        )}

        {answerSegments.length > 0 && (
          <section aria-label="回复" className={cn("ai-assistant-reply", hasProcess && "mt-6")}>
            {hasProcess && (
              <div className="ai-reply-heading">
                <MessageCircle aria-hidden className="h-3.5 w-3.5" />
                <span>{isWriting ? "正在回复" : "回复"}</span>
              </div>
            )}
            <div className="space-y-4">
              {answerSegments.map((segment, index) => (
                <MarkdownRenderer
                  key={segment.messageId ?? `text-${index}`}
                  content={segment.text}
                  streaming={isWriting && index === answerSegments.length - 1}
                  messageId={segment.messageId ?? messageId}
                  onRunPython={isStreaming ? undefined : onRunPython}
                  pythonRunRecords={pythonRunRecords}
                />
              ))}
            </div>
          </section>
        )}

        {isStreaming && !isWriting && !hasProcess && (
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

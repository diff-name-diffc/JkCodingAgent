import { motion } from "framer-motion";
import type {
  DispatcherMessageUsageStats,
  DispatcherToolArtifactRef,
  ModelCategory,
} from "../../types";
import {
  foldAssistantDrafts,
  type AssistantThinkingBlock,
  type AssistantTurnSegment,
} from "../dispatcher-chat/assistant-segments";
import type { ToolActivityItem } from "../dispatcher-chat/tool-activity";
import { cn } from "../../lib/cn";
import { ChatAvatar } from "./chat-avatar";
import { MarkdownRenderer } from "./markdown-renderer";
import { MessageActions } from "./message-actions";
import { ReasoningBlock } from "./reasoning-block";
import { ToolCallList } from "./tool-call-card";
import { formatTokenCountK } from "../dispatcher-chat/dispatcherChatUtils";

/**
 * Assistant message bubble for the refactored chat surface.
 *
 * Renders a full assistant turn: an avatar, a sequence of text + tool-summary
 * segments, optional thinking block, tool-call cards, and usage stats. The
 * segment / turn shape comes from buildDispatcherDisplayItems (unchanged) —
 * this component only re-styles it.
 *
 * Streaming tail is handled by <StreamingMessage /> (a slimmer variant that
 * renders the live segments from dispatcherSessionStore). This component is
 * for finalized turns.
 *
 * 草稿块协议：被后续正文取代的中间推理段不再作为独立灰块散落正文流，
 * 而是与模型思考链一起折叠进「思考过程」（与实时侧 StreamingMessage
 * 共用 foldAssistantDrafts）。
 */
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

export function AssistantMessage({
  segments,
  tools,
  thinking,
  usageStats,
  messageId,
  rowId,
  showAvatar = true,
  pythonRunRecords,
  onRunPython,
  onCopy,
  onRegenerate,
  onOpenArtifact,
  onOpenSubAgent,
  onConfigureModel,
  className,
}: AssistantMessageProps) {
  const { visible: visibleSegments, drafts } = foldAssistantDrafts(segments);
  const showReasoning = Boolean(thinking?.text?.trim()) || drafts.length > 0;

  const handleCopy = () => {
    // 复制仅取最终正文（草稿已折叠进思考过程，不属于答复正文）。
    const text = visibleSegments.map((s) => s.text).join("\n\n");
    if (onCopy) onCopy(text);
    else void navigator.clipboard.writeText(text);
  };

  return (
    <motion.div
      initial={{ opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.18, ease: [0.2, 0.8, 0.2, 1] }}
      className={cn("ai-assistant-message group relative", className)}
    >
      <ChatAvatar role="assistant" hidden={!showAvatar} className="absolute left-6 top-0.5" />

      <div className="min-w-0 pl-[60px]">
        {showReasoning && (
          <ReasoningBlock
            className="mb-2"
            text={thinking?.text ?? ""}
            drafts={drafts}
            elapsedMs={thinking?.elapsedMs ?? 0}
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

        {/* Segments：仅最终正文；中间推理草稿已折叠进上方思考过程。 */}
        <div className="space-y-2">
          {visibleSegments.map((segment, index) => (
            <MarkdownRenderer
              key={index}
              content={segment.text}
              messageId={segment.messageId ?? messageId}
              onRunPython={onRunPython}
              pythonRunRecords={pythonRunRecords}
            />
          ))}
        </div>

        <MessageActions
          tokenLabel={
            usageStats ? `${formatTokenCountK(usageStats.totalTokens)} tokens` : undefined
          }
          onCopy={handleCopy}
          onRegenerate={onRegenerate}
        />
      </div>
    </motion.div>
  );
}

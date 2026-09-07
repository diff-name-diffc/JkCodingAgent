import { motion } from "framer-motion";
import type { DispatcherMessageUsageStats, DispatcherToolArtifactRef } from "../../types";
import type { AssistantThinkingBlock, AssistantTurnSegment } from "../dispatcher-chat/assistant-segments";
import type { ToolActivityItem } from "../dispatcher-chat/tool-activity";
import { cn } from "../../lib/cn";
import { ChatAvatar } from "./chat-avatar";
import { MarkdownRenderer } from "./markdown-renderer";
import { MessageActions } from "./message-actions";
import { ReasoningBlock } from "./reasoning-block";
import { SupersededBlock } from "./superseded-block";
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
  className,
}: AssistantMessageProps) {
  const visibleSegments = segments.filter((s) => s.text.trim());

  const handleCopy = () => {
    const text = visibleSegments
      .filter((s) => !s.superseded)
      .map((s) => s.text)
      .join("\n\n");
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
      <ChatAvatar
        role="assistant"
        hidden={!showAvatar}
        className="absolute left-6 top-0.5"
      />

      <div className="min-w-0 pl-[60px]">
        {thinking?.text && (
          <ReasoningBlock
            className="mb-2"
            text={thinking.text}
            elapsedMs={thinking.elapsedMs}
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
          />
        )}

        {/* Segments */}
        <div className="space-y-2">
          {visibleSegments.map((segment, index) =>
            segment.superseded ? (
              <SupersededBlock key={index} text={segment.text} />
            ) : (
              <MarkdownRenderer
                key={index}
                content={segment.text}
                messageId={segment.messageId ?? messageId}
                onRunPython={onRunPython}
                pythonRunRecords={pythonRunRecords}
              />
            ),
          )}
        </div>

        <MessageActions
          tokenLabel={usageStats ? `${formatTokenCountK(usageStats.totalTokens)} tokens` : undefined}
          onCopy={handleCopy}
          onRegenerate={onRegenerate}
        />
      </div>
    </motion.div>
  );
}

import { motion } from "framer-motion";
import { Sparkles } from "lucide-react";
import type { AssistantThinkingBlock, AssistantTurnSegment } from "../dispatcherChatView";
import type { ToolActivityItem } from "../dispatcher-chat/tool-activity";
import type { DispatcherToolArtifactRef } from "../../types";
import { cn } from "../../lib/cn";
import { ChatAvatar } from "./chat-avatar";
import { MarkdownRenderer } from "./markdown-renderer";
import { ReasoningBlock } from "./reasoning-block";
import { ToolCallList } from "./tool-call-card";

/**
 * Live streaming assistant bubble.
 *
 * Renders the in-flight segments / tools / thinking from
 * dispatcherSessionStore's live state. The streaming flag is passed to the
 * tail segment's MarkdownRenderer only, so streamdown shows its built-in
 * blinking caret at the end of the text being generated (and re-renders just
 * that block); earlier segments render in static mode. The thinking block
 * keeps its own plain-text caret.
 */
export interface StreamingMessageProps {
  segments: AssistantTurnSegment[];
  tools?: ToolActivityItem[];
  thinking?: AssistantThinkingBlock | null;
  placeholder?: string | null;
  isStreaming: boolean;
  /** 连续 AI 消息分组中仅第一条显示头像（锚点位置保留，仅隐藏）。 */
  showAvatar?: boolean;
  onOpenArtifact?: (artifact: DispatcherToolArtifactRef) => void;
  onOpenSubAgent?: (tool: ToolActivityItem) => void;
  className?: string;
}

export function StreamingMessage({
  segments,
  tools,
  thinking,
  placeholder,
  isStreaming,
  showAvatar = true,
  onOpenArtifact,
  onOpenSubAgent,
  className,
}: StreamingMessageProps) {
  const visibleSegments = segments.filter((s) => s.text.trim());
  const hasContent = visibleSegments.length > 0 || (tools?.length ?? 0) > 0;

  return (
    <motion.div
      initial={{ opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.15 }}
      className={cn("ai-assistant-message relative", className)}
    >
      <ChatAvatar
        role="assistant"
        active={isStreaming}
        hidden={!showAvatar}
        className="absolute left-6 top-0.5"
      />

      <div className="min-w-0 pl-[60px]">
        {thinking?.text && (
          <ReasoningBlock
            className="mb-2"
            text={thinking.text}
            elapsedMs={thinking.elapsedMs}
            isStreaming={isStreaming}
          />
        )}

        {tools && tools.length > 0 && (
          <ToolCallList
            items={tools}
            className="mb-2"
            onOpenArtifact={onOpenArtifact}
            onOpenSubAgent={onOpenSubAgent}
          />
        )}

        <div className="space-y-2">
          {visibleSegments.map((segment, index) => (
            <MarkdownRenderer
              key={index}
              content={segment.text}
              streaming={isStreaming && index === visibleSegments.length - 1}
            />
          ))}
        </div>

        {!hasContent && placeholder && (
          <div className="flex items-center gap-2 text-sm text-muted-foreground">
            <Sparkles className="h-3.5 w-3.5 animate-pulse text-primary" />
            <span>{placeholder}</span>
          </div>
        )}
      </div>
    </motion.div>
  );
}

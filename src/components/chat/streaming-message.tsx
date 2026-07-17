import { motion } from "framer-motion";
import { Sparkles } from "lucide-react";
import type {
  AssistantThinkingBlock,
  AssistantTurnSegment,
} from "../dispatcherChatView";
import type { ToolActivityItem } from "../dispatcher-chat/tool-activity";
import type { DispatcherToolArtifactRef } from "../../types";
import { cn } from "../../lib/cn";
import { Avatar, AvatarFallback } from "../ui/avatar";
import { MarkdownRenderer } from "./markdown-renderer";
import { ToolCallList } from "./tool-call-card";

/**
 * Live streaming assistant bubble.
 *
 * Renders the in-flight segments / tools / thinking from
 * dispatcherSessionStore's live state. The streaming flag is passed through
 * to MarkdownRenderer so it throttles re-parses to ~7fps (unchanged
 * behaviour). A blinking caret is shown at the tail while text is still
 * arriving.
 */
export interface StreamingMessageProps {
  segments: AssistantTurnSegment[];
  tools?: ToolActivityItem[];
  thinking?: AssistantThinkingBlock | null;
  placeholder?: string | null;
  isStreaming: boolean;
  onOpenArtifact?: (artifact: DispatcherToolArtifactRef) => void;
  className?: string;
}

export function StreamingMessage({
  segments,
  tools,
  thinking,
  placeholder,
  isStreaming,
  onOpenArtifact,
  className,
}: StreamingMessageProps) {
  const visibleSegments = segments.filter((s) => s.text.trim());
  const hasContent = visibleSegments.length > 0 || (tools?.length ?? 0) > 0;

  return (
    <motion.div
      initial={{ opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.15 }}
      className={cn("flex items-start gap-3", className)}
    >
      <Avatar className="ai-assistant-avatar mt-0.5 h-8 w-8 border border-border bg-primary/10">
        <AvatarFallback>
          <Sparkles className={cn("h-3.5 w-3.5 text-primary", isStreaming && "animate-pulse")} />
        </AvatarFallback>
      </Avatar>

      <div className="min-w-0 flex-1">
        {thinking?.text && (
          <pre className="chat-scroll mb-2 max-h-40 overflow-auto rounded-md border border-border/60 bg-muted/40 p-3 font-mono text-[12px] leading-relaxed text-muted-foreground">
            {thinking.text}
            {isStreaming && <Caret />}
          </pre>
        )}

        {tools && tools.length > 0 && (
          <ToolCallList
            items={tools}
            className="mb-2"
            onOpenArtifact={onOpenArtifact}
          />
        )}

        <div className="space-y-2">
          {visibleSegments.map((segment, index) => (
            <MarkdownRenderer
              key={index}
              content={segment.text}
              streaming={isStreaming}
            />
          ))}
        </div>

        {!hasContent && placeholder && (
          <div className="flex items-center gap-2 text-sm text-muted-foreground">
            <Sparkles className="h-3.5 w-3.5 animate-pulse text-primary" />
            <span>{placeholder}</span>
          </div>
        )}

        {isStreaming && hasContent && (
          <div className="mt-1 h-4">
            <Caret />
          </div>
        )}
      </div>
    </motion.div>
  );
}

function Caret() {
  return (
    <motion.span
      aria-hidden
      animate={{ opacity: [1, 0.2, 1] }}
      transition={{ duration: 1, repeat: Infinity, ease: "easeInOut" }}
      className="inline-block translate-y-[-1px] text-primary"
    >
      ▋
    </motion.span>
  );
}

import * as React from "react";
import { motion } from "framer-motion";
import { Bot, Copy, RefreshCw, Sparkles } from "lucide-react";
import type { DispatcherMessageUsageStats, DispatcherToolArtifactRef } from "../../types";
import type { AssistantThinkingBlock, AssistantTurnSegment } from "../dispatcherChatView";
import type { ToolActivityItem } from "../dispatcher-chat/tool-activity";
import { cn } from "../../lib/cn";
import { Avatar, AvatarFallback } from "../ui/avatar";
import { Button } from "../ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "../ui/tooltip";
import { MarkdownRenderer } from "./markdown-renderer";
import { ToolCallList } from "./tool-call-card";

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
  pythonRunRecords,
  onRunPython,
  onCopy,
  onRegenerate,
  onOpenArtifact,
  onOpenSubAgent,
  className,
}: AssistantMessageProps) {
  const [thinkingOpen, setThinkingOpen] = React.useState(false);
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
      className={cn("ai-assistant-message group flex items-start gap-3", className)}
    >
      <Avatar className="ai-assistant-avatar mt-0.5 h-8 w-8 border border-border bg-primary/10">
        <AvatarFallback>
          <Bot className="h-4 w-4 text-primary" />
        </AvatarFallback>
      </Avatar>

      <div className="min-w-0 flex-1">
        {/* Thinking block — collapsible, dimmed */}
        {thinking?.text && (
          <div className="mb-2">
            <button
              onClick={() => setThinkingOpen((v) => !v)}
              className="mb-1 flex items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground"
            >
              <Sparkles className="h-3 w-3" />
              思考过程
              {thinking.elapsedMs > 0 && (
                <span className="text-muted-foreground/70">
                  · {(thinking.elapsedMs / 1000).toFixed(1)}s
                </span>
              )}
            </button>
            {thinkingOpen && (
              <pre className="chat-scroll max-h-64 overflow-auto rounded-md border border-border/60 bg-muted/40 p-3 font-mono text-[12px] leading-relaxed text-muted-foreground">
                {thinking.text}
              </pre>
            )}
          </div>
        )}

        {/* Tool calls */}
        {tools && tools.length > 0 && (
          <ToolCallList
            items={tools}
            className="mb-2"
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

        {/* Footer: usage + actions (actions reveal on hover) */}
        {(usageStats || onCopy || onRegenerate) && (
          <div className="mt-1.5 flex items-center gap-1 text-muted-foreground">
            {usageStats && (
              <span className="text-[11px]">
                {usageStats.totalTokens} tokens
                {usageStats.elapsedMs > 0 && ` · ${(usageStats.elapsedMs / 1000).toFixed(1)}s`}
              </span>
            )}
            <div className="flex-1" />
            <div className="flex items-center gap-1 opacity-0 transition-opacity duration-fast group-hover:opacity-100">
              {onCopy && (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      className="ai-message-action"
                      aria-label="复制回复"
                      onClick={handleCopy}
                    >
                      <Copy className="h-3.5 w-3.5" />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent>复制</TooltipContent>
                </Tooltip>
              )}
              {onRegenerate && (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      className="ai-message-action"
                      aria-label="重新生成"
                      onClick={onRegenerate}
                    >
                      <RefreshCw className="h-3.5 w-3.5" />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent>重新生成</TooltipContent>
                </Tooltip>
              )}
            </div>
          </div>
        )}
      </div>
    </motion.div>
  );
}

function SupersededBlock({ text }: { text: string }) {
  const [open, setOpen] = React.useState(false);
  return (
    <div className="rounded-md border border-dashed border-border/70 bg-muted/30">
      <button
        onClick={() => setOpen((v) => !v)}
        className="w-full px-3 py-1.5 text-left text-[11px] text-muted-foreground hover:text-foreground"
      >
        {open ? "收起中间推理" : "查看中间推理"}
      </button>
      {open && (
        <pre className="chat-scroll max-h-48 overflow-auto px-3 pb-2 font-mono text-[12px] leading-relaxed text-muted-foreground">
          {text}
        </pre>
      )}
    </div>
  );
}

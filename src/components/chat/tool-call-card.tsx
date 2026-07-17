import * as React from "react";
import { AnimatePresence, motion } from "framer-motion";
import { AlertTriangle, Bot, Check, ChevronRight, FileSearch, Loader2, Wrench } from "lucide-react";
import type { ToolActivityItem } from "../dispatcher-chat/tool-activity";
import type { DispatcherToolArtifactRef } from "../../types";
import { cn } from "../../lib/cn";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";

/**
 * Collapsible tool-call card for the Agent experience.
 *
 * Wraps the existing ToolActivityItem shape (status: planned | running |
 * completed) so it stays wire-compatible with dispatcherSessionStore's
 * liveToolCalls. The richer pending/running/success/error/cancelled statuses
 * are derived for display only — no data-model change.
 */

export type ToolCallDisplayStatus = "pending" | "running" | "success" | "error" | "cancelled";

function deriveStatus(item: ToolActivityItem): ToolCallDisplayStatus {
  switch (item.status) {
    case "planned":
      return "pending";
    case "running":
      return "running";
    case "completed":
      return "success";
    default:
      return "running";
  }
}

const STATUS_META: Record<
  ToolCallDisplayStatus,
  { label: string; icon: React.ReactNode; tone: "default" | "success" | "warning" | "destructive" }
> = {
  pending: {
    label: "待执行",
    icon: <Wrench className="h-3.5 w-3.5" />,
    tone: "default",
  },
  running: {
    label: "执行中",
    icon: <Loader2 className="h-3.5 w-3.5 animate-spin" />,
    tone: "default",
  },
  success: {
    label: "完成",
    icon: <Check className="h-3.5 w-3.5" />,
    tone: "success",
  },
  error: {
    label: "出错",
    icon: <AlertTriangle className="h-3.5 w-3.5" />,
    tone: "destructive",
  },
  cancelled: {
    label: "已取消",
    icon: <AlertTriangle className="h-3.5 w-3.5" />,
    tone: "warning",
  },
};

export interface ToolCallCardProps {
  item: ToolActivityItem;
  /** Optional duration in ms to show next to the status. */
  durationMs?: number;
  defaultExpanded?: boolean;
  className?: string;
  onOpenArtifact?: (artifact: DispatcherToolArtifactRef) => void;
  onOpenSubAgent?: (tool: ToolActivityItem) => void;
  /** Optional detail slot — e.g. an embedded sub-agent card or artifact. */
  detail?: React.ReactNode;
}

export function ToolCallCard({
  item,
  durationMs,
  defaultExpanded = false,
  className,
  onOpenArtifact,
  onOpenSubAgent,
  detail,
}: ToolCallCardProps) {
  const [expanded, setExpanded] = React.useState(defaultExpanded);
  const status = deriveStatus(item);
  const meta = STATUS_META[status];

  return (
    <div
      className={cn(
        "ai-tool-call-card rounded-lg border border-border bg-card/70 transition-colors",
        status === "running" && "border-primary/30",
        className,
      )}
    >
      <button
        onClick={() => setExpanded((v) => !v)}
        aria-expanded={expanded}
        className="ai-tool-call-trigger flex w-full items-center gap-2 px-3 py-2 text-left"
      >
        <motion.span
          animate={{ rotate: expanded ? 90 : 0 }}
          transition={{ duration: 0.15 }}
          className="text-muted-foreground"
        >
          <ChevronRight className="h-3.5 w-3.5" />
        </motion.span>
        <span className="text-muted-foreground">{meta.icon}</span>
        <span className="min-w-0 flex-1 truncate font-mono text-xs text-foreground">
          {item.name}
        </span>
        {durationMs != null && durationMs > 0 && (
          <span className="text-[11px] text-muted-foreground">
            {(durationMs / 1000).toFixed(1)}s
          </span>
        )}
        <Badge variant={meta.tone} className="shrink-0">
          {meta.label}
        </Badge>
      </button>

      <AnimatePresence initial={false}>
        {expanded && (
          <motion.div
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={{ duration: 0.18, ease: [0.2, 0.8, 0.2, 1] }}
            className="overflow-hidden border-t border-border/70"
          >
            <div className="space-y-2 px-3 py-2.5">
              {item.input && (
                <div>
                  <div className="mb-1 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
                    输入参数
                  </div>
                  <pre className="chat-scroll max-h-48 overflow-auto rounded-md bg-muted/60 p-2 font-mono text-[11px] leading-relaxed text-foreground">
                    {item.input}
                  </pre>
                </div>
              )}
              {item.displayText && (
                <div>
                  <div className="mb-1 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
                    结果摘要
                  </div>
                  <div className="rounded-md bg-muted/40 p-2 text-xs text-foreground">
                    {item.displayText}
                  </div>
                </div>
              )}
              {item.name === "call_sub_agent" && onOpenSubAgent && (
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="h-7"
                  onClick={() => onOpenSubAgent(item)}
                >
                  <Bot className="h-3.5 w-3.5" />
                  查看执行轨迹
                </Button>
              )}
              {detail}
              {item.detailRefs && item.detailRefs.length > 0 && (
                <div className="space-y-1.5">
                  {item.detailRefs.map((ref) => (
                    <button
                      key={ref.id}
                      type="button"
                      onClick={() => onOpenArtifact?.(ref)}
                      className="group flex w-full items-start gap-2 rounded-md border border-border/70 bg-background/60 px-2.5 py-2 text-left transition-colors hover:border-primary/40 hover:bg-primary/5"
                    >
                      <FileSearch className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground group-hover:text-primary" />
                      <span className="min-w-0 flex-1">
                        <span className="block truncate text-xs font-medium text-foreground">
                          {ref.title}
                        </span>
                        <span className="mt-0.5 block text-[11px] text-muted-foreground">
                          {ref.kind} · {ref.lineCount} 行 · {ref.charCount} 字符
                        </span>
                        {ref.preview && (
                          <span className="mt-1 line-clamp-2 block font-mono text-[11px] leading-relaxed text-muted-foreground">
                            {ref.preview}
                          </span>
                        )}
                      </span>
                    </button>
                  ))}
                </div>
              )}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/** Convenience: render a vertical list of tool calls. */
export function ToolCallList({
  items,
  className,
  onOpenArtifact,
  onOpenSubAgent,
}: {
  items: ToolActivityItem[];
  className?: string;
  onOpenArtifact?: (artifact: DispatcherToolArtifactRef) => void;
  onOpenSubAgent?: (tool: ToolActivityItem) => void;
}) {
  if (items.length === 0) return null;
  return (
    <div className={cn("space-y-1.5", className)}>
      {items.map((item) => (
        <ToolCallCard
          key={item.key}
          item={item}
          onOpenArtifact={onOpenArtifact}
          onOpenSubAgent={onOpenSubAgent}
        />
      ))}
    </div>
  );
}

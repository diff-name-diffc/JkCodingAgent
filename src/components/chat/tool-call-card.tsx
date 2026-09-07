import * as React from "react";
import { AnimatePresence, motion } from "framer-motion";
import { Bot, Check, ChevronDown, Clock3, FileSearch, Loader2, X } from "lucide-react";
import type { ToolActivityItem, ToolCallStatus } from "../dispatcher-chat/tool-activity";
import {
  formatToolActivitySummary,
  summarizeToolActivity,
} from "../dispatcher-chat/tool-activity-summary";
import type { DispatcherToolArtifactRef } from "../../types";
import { cn } from "../../lib/cn";
import { highlightCodeToHtml } from "../../utils/shiki";
import { useIsDarkTheme } from "../../hooks/useIsDarkTheme";
import { Button } from "../ui/button";
import { StatusPill } from "../detail/StatusPill";
import { GraphPlanCard } from "../graph/GraphPlanCard";
import { parseGraphPlanId } from "../graph/graph-utils";
import { ToolRunTrace } from "./tool-run-trace";

const MAX_COLLAPSED_OUTPUT_LINES = 20;

interface ToolCallCardProps {
  item: ToolActivityItem;
  defaultExpanded?: boolean;
  className?: string;
  onOpenArtifact?: (artifact: DispatcherToolArtifactRef) => void;
  onOpenSubAgent?: (tool: ToolActivityItem) => void;
  detail?: React.ReactNode;
}

function ToolCallCard({
  item,
  defaultExpanded = false,
  className,
  onOpenArtifact,
  onOpenSubAgent,
  detail,
}: ToolCallCardProps) {
  const [expanded, setExpanded] = React.useState(defaultExpanded);
  // submit_graph 收口工具：从输出文本解析 plan_id，卡片下方内联图计划卡。
  const graphPlanId =
    item.name === "submit_graph" && typeof item.output === "string"
      ? parseGraphPlanId(item.output)
      : null;

  return (
    <div
      className={cn(
        "ai-tool-call-card rounded-lg border bg-card/70",
        item.status === "running" && !item.planned && "ai-tool-call-card--running border-primary/30",
        item.status === "error" && "ai-tool-call-card--error border-destructive/60",
        className,
      )}
    >
      <button
        type="button"
        onClick={() => setExpanded((value) => !value)}
        aria-expanded={expanded}
        className="ai-tool-call-trigger flex w-full items-center gap-2 px-3 py-2 text-left"
      >
        <span className="min-w-0 flex-1 truncate font-mono text-[13px] font-medium text-foreground">
          {item.name}
        </span>
        <StatusPill
          domain="tool"
          status={item.planned ? "planned" : item.status}
          className="shrink-0"
        />
        {item.durationMs != null && (
          <span className="shrink-0 tabular-nums text-[11px] text-muted-foreground">
            {formatDuration(item.durationMs)}
          </span>
        )}
        <ChevronDown
          aria-hidden
          className={cn(
            "h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform duration-fast",
            !expanded && "-rotate-90",
          )}
        />
      </button>

      {graphPlanId && <GraphPlanCard planId={graphPlanId} />}

      <AnimatePresence initial={false}>
        {expanded && (
          <motion.div
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={{ duration: 0.18, ease: [0.2, 0.8, 0.2, 1] }}
            className="overflow-hidden border-t border-border/70"
          >
            <div className="space-y-3 px-3 py-3">
              <ToolRunTrace item={item} active={expanded} />
              {item.input != null && <DataSection label="输入" value={item.input} />}
              {item.output != null && item.output !== item.errorText && (
                <DataSection
                  label={isCompressedResult(item.resultMode) ? "输出 · 回传模型" : "输出"}
                  value={item.output}
                  collapsible
                />
              )}
              {item.errorText && (
                <div className="rounded-md border border-destructive/30 bg-destructive/5 p-2.5 font-mono text-[11px] leading-relaxed text-destructive">
                  {item.errorText}
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

function DataSection({
  label,
  value,
  collapsible = false,
}: {
  label: "输入" | "输出" | "输出 · 回传模型";
  value: unknown;
  collapsible?: boolean;
}) {
  const [showAll, setShowAll] = React.useState(false);
  const content = serializeData(value);
  const lines = content.split("\n");
  const isLong = collapsible && lines.length > MAX_COLLAPSED_OUTPUT_LINES;
  const visibleContent =
    isLong && !showAll ? lines.slice(0, MAX_COLLAPSED_OUTPUT_LINES).join("\n") : content;

  return (
    <section>
      <div className="mb-1.5 text-[10px] font-semibold uppercase tracking-[0.12em] text-muted-foreground">
        {label}
      </div>
      <JsonCode value={visibleContent} />
      {isLong && (
        <button
          type="button"
          onClick={() => setShowAll((value) => !value)}
          className="mt-1.5 text-[11px] font-medium text-primary hover:text-primary-hover"
          aria-expanded={showAll}
        >
          {showAll ? "收起" : `展开全部（${lines.length} 行）`}
        </button>
      )}
    </section>
  );
}

function isCompressedResult(resultMode: ToolActivityItem["resultMode"]): boolean {
  return (
    resultMode === "summary" ||
    resultMode === "conservative_summary" ||
    resultMode === "intent_compressed" ||
    resultMode === "structured_fallback"
  );
}

function JsonCode({ value }: { value: string }) {
  const [highlighted, setHighlighted] = React.useState<string | null>(null);
  const isDark = useIsDarkTheme();

  React.useEffect(() => {
    let active = true;
    // 主题切换时先回退纯文本渲染：旧主题的高亮 HTML 携带固定前景/背景色，
    // 保留到新主题 resolve 为止会出现样式错乱。
    setHighlighted(null);
    highlightCodeToHtml(value, "json", isDark)
      .then((html) => {
        if (active) setHighlighted(html);
      })
      .catch(() => {
        if (active) setHighlighted(null);
      });
    return () => {
      active = false;
    };
  }, [value, isDark]);

  if (!highlighted) {
    return (
      <pre className="chat-scroll overflow-auto rounded-md bg-muted/60 p-2.5 font-mono text-[11px] leading-[1.55] text-foreground">
        {value}
      </pre>
    );
  }

  return (
    <div
      className="ai-tool-call-code chat-scroll overflow-auto rounded-md bg-muted/60 font-mono text-[11px] leading-[1.55]"
      dangerouslySetInnerHTML={{ __html: highlighted }}
    />
  );
}

export interface ToolCallListProps {
  items: ToolActivityItem[];
  className?: string;
  onOpenArtifact?: (artifact: DispatcherToolArtifactRef) => void;
  onOpenSubAgent?: (tool: ToolActivityItem) => void;
}

export function ToolCallList({
  items,
  className,
  onOpenArtifact,
  onOpenSubAgent,
}: ToolCallListProps) {
  const aggregated = items.length >= 3;
  const [expanded, setExpanded] = React.useState(!aggregated);
  const wasAggregated = React.useRef(aggregated);
  const summary = React.useMemo(() => summarizeToolActivity(items), [items]);

  React.useEffect(() => {
    if (aggregated && !wasAggregated.current) setExpanded(false);
    if (!aggregated) setExpanded(true);
    wasAggregated.current = aggregated;
  }, [aggregated]);

  if (items.length === 0) return null;

  const renderRow = (item: ToolActivityItem, index: number, list: ToolActivityItem[]) => (
    <div key={item.id} className="flex items-stretch gap-2">
      <div className="relative w-6 shrink-0" aria-hidden>
        {index < list.length - 1 && <span className="ai-tool-call-line" />}
        <TimelineNode status={item.status} planned={item.planned} />
      </div>
      <ToolCallCard
        item={item}
        className="mb-2 min-w-0 flex-1"
        onOpenArtifact={onOpenArtifact}
        onOpenSubAgent={onOpenSubAgent}
      />
    </div>
  );

  // UI-12：聚合收起时，失败/运行中/等待卡固定露出在摘要行下方——
  // 「折叠不隐藏错误」「失败与待处理状态无需展开即可看见」。
  const pinnedItems =
    aggregated && !expanded ? items.filter((item) => item.status !== "success") : [];
  const allSettled = summary.failed === 0 && summary.running === 0 && summary.planned === 0;

  return (
    <div className={className}>
      {aggregated && (
        <button
          type="button"
          onClick={() => setExpanded((value) => !value)}
          aria-expanded={expanded}
          className="ai-tool-call-summary flex w-full items-center gap-2 rounded-lg border border-border bg-card/70 px-3 py-2 text-left text-xs text-foreground hover:border-primary/30 hover:bg-muted/40"
        >
          <span aria-hidden className="text-sm">
            ⚙
          </span>
          <span className="min-w-0 flex-1 truncate">{formatToolActivitySummary(summary)}</span>
          <span className="ai-tool-summary-pills flex shrink-0 items-center gap-1">
            {summary.failed > 0 && (
              <StatusPill domain="tool" status="error" label={`失败 ${summary.failed}`} />
            )}
            {summary.planned > 0 && (
              <StatusPill domain="tool" status="planned" label={`等待 ${summary.planned}`} />
            )}
            {summary.running > 0 && (
              <StatusPill domain="tool" status="running" label={`执行中 ${summary.running}`} />
            )}
            {allSettled && <StatusPill domain="tool" status="success" label="成功" />}
          </span>
          <ChevronDown
            aria-hidden
            className={cn(
              "h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform duration-fast",
              !expanded && "-rotate-90",
            )}
          />
        </button>
      )}

      {pinnedItems.length > 0 && (
        <div className="ai-tool-call-pinned space-y-0 pt-2">
          {pinnedItems.map((item, index) => renderRow(item, index, pinnedItems))}
        </div>
      )}

      <AnimatePresence initial={false}>
        {expanded && (
          <motion.div
            initial={aggregated ? { height: 0, opacity: 0 } : false}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={{ duration: 0.2, ease: [0.2, 0.8, 0.2, 1] }}
            className={cn("overflow-hidden", aggregated && "pt-2")}
          >
            <div className="space-y-0">{items.map((item, index) => renderRow(item, index, items))}</div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

function TimelineNode({ status, planned }: { status: ToolCallStatus; planned?: boolean }) {
  const className = "h-3.5 w-3.5";
  const variant = planned ? "planned" : status;
  return (
    <span className={cn("ai-tool-call-node", `ai-tool-call-node--${variant}`)}>
      {planned && <Clock3 className={className} />}
      {!planned && status === "running" && <Loader2 className={cn(className, "animate-spin")} />}
      {status === "success" && <Check className={className} />}
      {status === "error" && <X className={className} />}
    </span>
  );
}

function serializeData(value: unknown): string {
  if (typeof value === "string") {
    try {
      return JSON.stringify(JSON.parse(value), null, 2);
    } catch {
      return value;
    }
  }
  return JSON.stringify(value, null, 2) ?? String(value);
}

function formatDuration(durationMs: number): string {
  if (durationMs < 1000) return `${Math.round(durationMs)}ms`;
  return `${(durationMs / 1000).toFixed(1)}s`;
}

import * as React from "react";
import { Bot, Check, ChevronRight, Clock3, FileSearch, Loader2, X } from "lucide-react";
import type { ToolActivityItem } from "../dispatcher-chat/tool-activity";
import type { DispatcherToolArtifactRef, ModelCategory } from "../../types";
import {
  inferModelNotConfiguredCategory,
  isModelNotConfiguredError,
} from "../../lib/run-error-classify";
import { cn } from "../../lib/cn";
import { Button } from "../ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "../ui/collapsible";
import { WorkflowPlanCard } from "../workflow/WorkflowPlanCard";
import { BrowserActivityFeed, BrowserTraceView } from "../browser/BrowserTraceView";
import { ToolRunTrace } from "./tool-run-trace";
import { ToolCallData } from "./tool-call-data";
import {
  formatToolDuration,
  presentToolActivity,
  toolActivityErrorSummary,
  toolWorkflowPlanId,
} from "./tool-activity-presentation";

export interface ToolActivityActions {
  onOpenArtifact?: (artifact: DispatcherToolArtifactRef) => void;
  onOpenSubAgent?: (tool: ToolActivityItem) => void;
  onConfigureModel?: (category?: ModelCategory) => void;
}

interface ToolActivityRowProps extends ToolActivityActions {
  item: ToolActivityItem;
  expanded: boolean;
  onExpandedChange: (id: string, expanded: boolean) => void;
}

export const ToolActivityRow = React.memo(function ToolActivityRow({
  item,
  expanded,
  onExpandedChange,
  onOpenArtifact,
  onOpenSubAgent,
  onConfigureModel,
}: ToolActivityRowProps) {
  const presentation = React.useMemo(
    () => presentToolActivity({ name: item.name, input: item.input }),
    [item.name, item.input],
  );
  const workflowPlanId = toolWorkflowPlanId(item);
  const errorSummary = toolActivityErrorSummary(item);
  const isBrowserTool = item.name.startsWith("browser_");
  const status = item.planned
    ? "等待"
    : { running: "进行中", success: "完成", error: "失败" }[item.status];
  const configureError =
    item.errorText ??
    (item.status === "error" && typeof item.output === "string" ? item.output : "");

  return (
    <Collapsible
      open={expanded}
      onOpenChange={(open) => {
        onExpandedChange(item.id, open);
      }}
      className={cn(
        "ai-tool-call-card rounded-md",
        item.status === "error" && "ai-tool-call-card--error",
      )}
    >
      <CollapsibleTrigger asChild>
        <button
          type="button"
          className="ai-tool-call-trigger group flex min-h-9 w-full items-center gap-2 rounded-md px-2 py-1.5 text-left focus-visible:outline-2 focus-visible:outline-[var(--border-focus)]"
          aria-label={`${presentation.action}${presentation.target ? `：${presentation.target}` : ""}，${status}，${expanded ? "收起" : "查看"}详情`}
        >
          <ActivityStatus item={item} />
          <span className="shrink-0 text-xs font-medium text-foreground">
            {presentation.action}
          </span>
          {presentation.target && (
            <span
              className="min-w-0 flex-1 truncate text-xs text-muted-foreground"
              title={presentation.target}
            >
              {presentation.target}
            </span>
          )}
          {!presentation.target && <span className="flex-1" />}
          <span
            className={cn(
              "shrink-0 text-[11px]",
              item.status === "error" ? "text-destructive" : "text-muted-foreground",
            )}
          >
            {item.status === "success" && !item.planned && item.durationMs != null
              ? formatToolDuration(item.durationMs)
              : status}
          </span>
          <ChevronRight
            aria-hidden
            className={cn(
              "h-3 w-3 shrink-0 text-muted-foreground transition-transform duration-fast motion-reduce:transition-none",
              expanded && "rotate-90",
            )}
          />
        </button>
      </CollapsibleTrigger>

      {errorSummary && (
        <p className="m-0 px-8 pb-2 text-xs leading-relaxed break-words text-destructive">
          {errorSummary}
        </p>
      )}
      {configureError && onConfigureModel && isModelNotConfiguredError(configureError) && (
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="mb-2 ml-8"
          onClick={() =>
            onConfigureModel(inferModelNotConfiguredCategory(configureError) ?? undefined)
          }
        >
          配置模型
        </Button>
      )}
      {workflowPlanId && (
        <div className="pb-2 pl-8 pr-2">
          <WorkflowPlanCard planId={workflowPlanId} sessionId={item.workspaceId} />
        </div>
      )}
      {isBrowserTool && (item.browserActivity?.length ?? 0) > 0 && (
        <div className="pb-2 pl-8 pr-2">
          <BrowserActivityFeed
            lines={item.browserActivity ?? []}
            expanded={expanded}
            active={item.status === "running"}
          />
        </div>
      )}

      <CollapsibleContent>
        <div className="mb-2 ml-3 mr-2 space-y-3 border-l border-border py-2 pl-4 pr-1">
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-muted-foreground">
            <code className="font-mono">{item.name}</code>
            <span>
              {status}
              {item.durationMs != null ? ` · ${formatToolDuration(item.durationMs)}` : ""}
            </span>
          </div>
          {isBrowserTool && item.workspaceId && <BrowserTraceView sessionId={item.workspaceId} />}
          <ToolRunTrace item={item} active={expanded} />
          {item.input != null && (
            <ToolCallData label="输入" value={item.input} persistKey={`showall:${item.id}:input`} />
          )}
          {item.output != null && item.output !== item.errorText && (
            <ToolCallData
              label={isCompressedResult(item.resultMode) ? "输出 · 回传模型" : "输出"}
              value={item.output}
              persistKey={`showall:${item.id}:output`}
            />
          )}
          {item.errorText && (
            <pre className="chat-scroll max-h-64 overflow-auto whitespace-pre-wrap break-words rounded-md bg-destructive/5 p-2.5 font-mono text-[11px] leading-relaxed text-destructive">
              {item.errorText}
            </pre>
          )}
          {item.name === "call_sub_agent" && onOpenSubAgent && (
            <Button type="button" variant="outline" size="sm" onClick={() => onOpenSubAgent(item)}>
              <Bot className="h-3.5 w-3.5" />
              查看执行轨迹
            </Button>
          )}
          {item.detailRefs && item.detailRefs.length > 0 && (
            <div className="space-y-1.5">
              {item.detailRefs.map((artifact) => (
                <button
                  key={artifact.id}
                  type="button"
                  onClick={() => onOpenArtifact?.(artifact)}
                  disabled={!onOpenArtifact}
                  className="group flex w-full items-start gap-2 rounded-md border border-border/70 bg-background/60 px-2.5 py-2 text-left transition-colors enabled:hover:border-primary/40 enabled:hover:bg-primary/5 disabled:cursor-default"
                >
                  <FileSearch
                    aria-hidden
                    className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground group-hover:text-primary"
                  />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-xs font-medium text-foreground">
                      {artifact.title}
                    </span>
                    <span className="mt-0.5 block text-[11px] text-muted-foreground">
                      {artifact.kind} · {artifact.lineCount} 行 · {artifact.charCount} 字符
                    </span>
                    {artifact.preview && (
                      <span className="mt-1 line-clamp-2 block font-mono text-[11px] leading-relaxed text-muted-foreground">
                        {artifact.preview}
                      </span>
                    )}
                  </span>
                </button>
              ))}
            </div>
          )}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
});

function ActivityStatus({ item }: { item: ToolActivityItem }) {
  const Icon = item.planned ? Clock3 : { running: Loader2, success: Check, error: X }[item.status];
  return (
    <Icon
      aria-hidden
      className={cn(
        "h-3.5 w-3.5 shrink-0",
        item.status === "error" ? "text-destructive" : "text-muted-foreground",
        item.status === "running" &&
          !item.planned &&
          "animate-spin text-primary motion-reduce:animate-none",
      )}
    />
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

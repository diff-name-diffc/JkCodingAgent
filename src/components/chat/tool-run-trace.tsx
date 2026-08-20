import * as React from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check, Loader2, X } from "lucide-react";

import type { DispatcherToolRunRecord } from "../../types";
import { cn } from "../../lib/cn";
import {
  mergeToolRunRecords,
  toolRunStatusToCallStatus,
  type ToolActivityItem,
} from "../dispatcher-chat/tool-activity";
import { Badge } from "../ui/badge";

interface PersistedTraceState {
  key: string;
  status: "loading" | "loaded" | "error";
  runs: DispatcherToolRunRecord[];
  error?: string;
}

export function ToolRunTrace({ item, active }: { item: ToolActivityItem; active: boolean }) {
  const [persistedTrace, setPersistedTrace] = React.useState<PersistedTraceState | null>(null);
  const traceKey = `${item.workspaceId ?? ""}:${item.id}:${item.runId ?? ""}`;
  // live 事件可能从中途开始订阅；即使已经收到部分 child run，也要补读持久化
  // 树并按 run id 合并，不能把“至少一条 live 记录”误当成“轨迹完整”。
  const shouldLoad = item.name === "run_tool_program";
  const runs = React.useMemo(
    () =>
      mergeToolRunRecords(
        persistedTrace?.key === traceKey ? persistedTrace.runs : [],
        item.toolRuns ?? [],
      ),
    [item.toolRuns, persistedTrace, traceKey],
  );

  React.useEffect(() => {
    if (!active || !item.workspaceId || !shouldLoad) return;

    let mounted = true;
    setPersistedTrace({ key: traceKey, status: "loading", runs: [] });
    void invoke<DispatcherToolRunRecord[]>("dispatcher_get_tool_run_tree", {
      workspaceId: item.workspaceId,
      toolCallId: item.id,
      rootRunId: item.runId ?? null,
    })
      .then((records) => {
        if (!mounted) return;
        setPersistedTrace({
          key: traceKey,
          status: "loaded",
          runs: records,
        });
      })
      .catch((error: unknown) => {
        if (!mounted) return;
        setPersistedTrace({
          key: traceKey,
          status: "error",
          runs: [],
          error: error instanceof Error ? error.message : String(error),
        });
      });
    return () => {
      mounted = false;
    };
  }, [active, item.id, item.runId, item.workspaceId, shouldLoad, traceKey]);

  const state = persistedTrace?.key === traceKey ? persistedTrace : null;
  return (
    <ToolRunTree
      runs={runs}
      rootRunId={item.runId}
      rootToolCallId={item.id}
      loading={state?.status === "loading"}
      error={state?.error}
    />
  );
}

function ToolRunTree({
  runs,
  rootRunId,
  rootToolCallId,
  loading,
  error,
}: {
  runs: DispatcherToolRunRecord[];
  rootRunId?: string;
  rootToolCallId: string;
  loading: boolean;
  error?: string;
}) {
  const byId = new Map(runs.map((run) => [run.id, run] as const));
  const root = rootRunId
    ? byId.get(rootRunId)
    : runs.find((run) => !run.parentRunId && run.toolCallId === rootToolCallId);
  const childrenByParent = new Map<string, DispatcherToolRunRecord[]>();
  for (const run of runs) {
    if (!run.parentRunId) continue;
    const siblings = childrenByParent.get(run.parentRunId) ?? [];
    siblings.push(run);
    childrenByParent.set(run.parentRunId, siblings);
  }
  const topLevelRuns = root
    ? (childrenByParent.get(root.id) ?? [])
    : runs.filter((run) => run.parentRunId && !byId.has(run.parentRunId));
  const nestedRunCount = runs.filter((run) => run.parentRunId).length;

  if (topLevelRuns.length === 0 && !loading && !error) return null;

  return (
    <section className="overflow-hidden rounded-md border border-border/70 bg-muted/20">
      <div className="flex items-center gap-2 border-b border-border/60 px-2.5 py-2">
        <span className="text-[10px] font-semibold uppercase tracking-[0.12em] text-muted-foreground">
          运行时步骤
        </span>
        {nestedRunCount > 0 && (
          <span className="rounded bg-muted px-1.5 py-0.5 font-mono text-[10px] tabular-nums text-muted-foreground">
            {nestedRunCount}
          </span>
        )}
        {loading && (
          <span className="ml-auto flex items-center gap-1 text-[10px] text-muted-foreground">
            <Loader2 className="h-3 w-3 animate-spin" />
            恢复轨迹
          </span>
        )}
      </div>

      {error && (
        <div className="border-b border-destructive/20 bg-destructive/5 px-2.5 py-2 text-[11px] text-destructive">
          轨迹加载失败：{error}
        </div>
      )}

      {topLevelRuns.length > 0 && (
        <div className="space-y-1 p-2">
          {topLevelRuns.map((run) => (
            <ToolRunNode key={run.id} run={run} childrenByParent={childrenByParent} />
          ))}
        </div>
      )}
    </section>
  );
}

function ToolRunNode({
  run,
  childrenByParent,
}: {
  run: DispatcherToolRunRecord;
  childrenByParent: Map<string, DispatcherToolRunRecord[]>;
}) {
  const status = toolRunStatusToCallStatus(run.status);
  const children = childrenByParent.get(run.id) ?? [];

  return (
    <div>
      <div
        className={cn(
          "rounded border bg-background/70 px-2.5 py-2",
          status === "error" ? "border-destructive/30" : "border-border/60",
        )}
      >
        <div className="flex min-w-0 items-center gap-2">
          <span
            className={cn(
              "flex h-4 w-4 shrink-0 items-center justify-center rounded-full",
              status === "success" && "bg-success/10 text-success",
              status === "error" && "bg-destructive/10 text-destructive",
              status === "running" && "bg-primary/10 text-primary",
            )}
          >
            {status === "running" && (
              <Loader2 className={cn("h-3 w-3", run.status === "running" && "animate-spin")} />
            )}
            {status === "success" && <Check className="h-3 w-3" />}
            {status === "error" && <X className="h-3 w-3" />}
          </span>
          <code
            title={run.stepId ?? undefined}
            className="max-w-32 shrink-0 truncate rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground"
          >
            {run.stepId || `step-${run.sequence}`}
          </code>
          <span className="min-w-0 flex-1 truncate font-mono text-[11px] font-medium text-foreground">
            {run.toolName}
          </span>
          <Badge
            variant={
              status === "success" ? "success" : status === "error" ? "destructive" : "default"
            }
            className="shrink-0 px-1.5 py-0 text-[9px]"
          >
            {statusLabel(run.status)}
          </Badge>
          <span className="w-12 shrink-0 text-right font-mono text-[10px] tabular-nums text-muted-foreground">
            {status === "running" ? "—" : formatDuration(run.durationMs)}
          </span>
        </div>
        {(run.errorMessage || run.errorKind) && (
          <div className="mt-1.5 border-l-2 border-destructive/30 pl-2 font-mono text-[10px] leading-relaxed text-destructive">
            {run.errorKind && <span className="mr-1 opacity-70">[{run.errorKind}]</span>}
            {run.errorMessage || "工具步骤执行失败"}
          </div>
        )}
      </div>

      {children.length > 0 && (
        <div className="ml-3 mt-1 space-y-1 border-l border-border/70 pl-3">
          {children.map((child) => (
            <ToolRunNode key={child.id} run={child} childrenByParent={childrenByParent} />
          ))}
        </div>
      )}
    </div>
  );
}

function statusLabel(status: string): string {
  const labels: Record<string, string> = {
    planned: "等待",
    running: "执行中",
    succeeded: "成功",
    recoverable_error: "可恢复错误",
    fatal_error: "致命错误",
    internal_error: "内部错误",
    cancelled: "已取消",
    failed: "失败",
  };
  return labels[status] ?? status;
}

function formatDuration(durationMs: number): string {
  if (durationMs < 1000) return `${Math.round(durationMs)}ms`;
  return `${(durationMs / 1000).toFixed(1)}s`;
}

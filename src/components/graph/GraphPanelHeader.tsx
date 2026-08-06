import { useMemo } from "react";
import { Play, RotateCcw, Square, X } from "lucide-react";
import type { GraphDefinition, GraphNodeStatus, GraphPlanRecord, GraphPlanStatus } from "../../types";
import { cn } from "../../lib/cn";
import { Button } from "../ui/button";
import { PLAN_STATUS_META, computeGraphLayers } from "./graph-utils";

interface GraphPanelHeaderProps {
  plan: GraphPlanRecord | null;
  definition: GraphDefinition | null;
  planStatus: GraphPlanStatus;
  paused: boolean;
  actionPending: boolean;
  statusByNodeId: Map<string, GraphNodeStatus>;
  onStart: (mode: "full" | "resume") => void;
  onResumeCheckpoint: () => void;
  onCancel: () => void;
  onClose: () => void;
}

/**
 * 图编排面板两层头部：标题/状态/验收/操作 + 任务统计/整体进度。
 * 操作语义：draft →「确认执行」(full)；failed/cancelled →「从断点继续」(resume，主)
 * +「完整重跑」(full)；completed →「完整重跑」(full)；running →「停止」，
 * 高危写检查点暂停时另显「继续执行」(graph_run_resume)。
 */
export function GraphPanelHeader({
  plan,
  definition,
  planStatus,
  paused,
  actionPending,
  statusByNodeId,
  onStart,
  onResumeCheckpoint,
  onCancel,
  onClose,
}: GraphPanelHeaderProps) {
  const statusMeta = PLAN_STATUS_META[planStatus];
  const canStart = planStatus === "draft";
  const canResumeRun = planStatus === "failed" || planStatus === "cancelled";
  const canFullRerun = planStatus === "completed";
  const canCancel = planStatus === "running";

  // ── 头部统计：任务数 / 最大并行（最大层宽）/ 状态计数 / 整体进度 ──
  const stats = useMemo(() => {
    const nodes = definition?.nodes ?? [];
    const layers = definition ? computeGraphLayers(definition) : [];
    const maxParallel = layers.reduce((max, layer) => Math.max(max, layer.length), 0);
    const counts = { running: 0, succeeded: 0, failed: 0, skipped: 0, cancelled: 0, settled: 0 };
    for (const node of nodes) {
      const status = statusByNodeId.get(node.id) ?? "pending";
      if (status === "running") counts.running += 1;
      if (status === "succeeded") counts.succeeded += 1;
      if (status === "failed") counts.failed += 1;
      if (status === "skipped") counts.skipped += 1;
      if (status === "cancelled") counts.cancelled += 1;
    }
    // 进度只统计真正产出执行结论的节点（成功/失败）：把 cancelled/skipped
    // 计入 settled 会让被取消的运行显示 100%，掩盖任务并未真正完成的事实。
    counts.settled = counts.succeeded + counts.failed;
    const total = nodes.length;
    const progress = total > 0 ? Math.round((counts.settled / total) * 100) : 0;
    const codingNodes = nodes.filter((node) => node.baseToolGroup === "coding").length;
    // 粗估 token：task 字符数 / 4（仅启动前参考，标注「估算」）。
    const estimatedTokens = Math.round(
      nodes.reduce((sum, node) => sum + node.task.length, 0) / 4,
    );
    return { total, maxParallel, progress, codingNodes, estimatedTokens, ...counts };
  }, [definition, statusByNodeId]);

  // 最近一次运行的验收结论（runs 按 attemptNo 倒序，取首个）。
  // 后端已把「尚未/未能验收」统一为 unknown（空串会被读取层归一），
  // 运行中的运行还没有验收结论，不显示徽章。
  const latestVerdict = useMemo(() => {
    const run = plan?.runs?.[0];
    if (!run || run.status === "running" || !run.verdictStatus) return null;
    const meta: Record<string, { label: string; className: string }> = {
      pass: { label: "验收通过", className: "ai-graph-chip--node-succeeded" },
      partial: { label: "部分达成", className: "ai-graph-chip--draft" },
      fail: { label: "验收未通过", className: "ai-graph-chip--failed" },
      unknown: { label: "未能验收", className: "ai-graph-chip--cancelled" },
    };
    const found = meta[run.verdictStatus];
    return found ? { ...found, reason: run.verdictReason } : null;
  }, [plan]);

  return (
    <header className="ai-graph-panel-header">
      <div className="ai-graph-panel-header-top">
        <div className="ai-graph-panel-heading">
          <span className="ai-graph-panel-title">{plan?.title ?? "执行图"}</span>
          {plan?.summary && (
            <span className="ai-graph-panel-summary" title={plan.summary}>
              {plan.summary}
            </span>
          )}
        </div>
        <span className={cn("ai-graph-chip", statusMeta.className)}>{statusMeta.label}</span>
        {latestVerdict && (
          <span className={cn("ai-graph-chip", latestVerdict.className)} title={latestVerdict.reason}>
            {latestVerdict.label}
          </span>
        )}
        {canStart && (
          <Button size="sm" onClick={() => onStart("full")} disabled={actionPending || !plan}>
            <Play className="h-3.5 w-3.5" />
            确认执行
          </Button>
        )}
        {canResumeRun && (
          <>
            <Button size="sm" onClick={() => onStart("resume")} disabled={actionPending || !plan}>
              <Play className="h-3.5 w-3.5" />
              从断点继续
            </Button>
            <Button
              size="sm"
              variant="outline"
              onClick={() => onStart("full")}
              disabled={actionPending || !plan}
            >
              <RotateCcw className="h-3.5 w-3.5" />
              完整重跑
            </Button>
          </>
        )}
        {canFullRerun && (
          <Button
            size="sm"
            variant="outline"
            onClick={() => onStart("full")}
            disabled={actionPending || !plan}
          >
            <RotateCcw className="h-3.5 w-3.5" />
            完整重跑
          </Button>
        )}
        {canCancel && paused && (
          <Button size="sm" onClick={onResumeCheckpoint} disabled={actionPending}>
            <Play className="h-3.5 w-3.5" />
            继续执行
          </Button>
        )}
        {canCancel && (
          <Button
            size="sm"
            variant="destructive"
            onClick={onCancel}
            disabled={actionPending}
          >
            <Square className="h-3.5 w-3.5" />
            停止
          </Button>
        )}
        <Button variant="ghost" size="icon-sm" aria-label="关闭执行图面板" onClick={onClose}>
          <X className="h-4 w-4" />
        </Button>
      </div>
      <div className="ai-graph-panel-header-stats">
        <span className="ai-graph-stat">任务 {stats.total}</span>
        <span className="ai-graph-stat">并行 {stats.maxParallel}</span>
        {stats.codingNodes > 0 && (
          <span
            className="ai-graph-stat ai-graph-stat--failed"
            title="这些节点可能修改文件或执行命令"
          >
            写节点 {stats.codingNodes}
          </span>
        )}
        {stats.estimatedTokens > 0 && (
          <span className="ai-graph-stat" title="按任务描述长度粗估，仅供参考">
            ≈{stats.estimatedTokens} tokens（估算）
          </span>
        )}
        {stats.running > 0 && (
          <span className="ai-graph-stat ai-graph-stat--running">运行中 {stats.running}</span>
        )}
        {stats.succeeded > 0 && (
          <span className="ai-graph-stat ai-graph-stat--succeeded">成功 {stats.succeeded}</span>
        )}
        {stats.failed > 0 && (
          <span className="ai-graph-stat ai-graph-stat--failed">失败 {stats.failed}</span>
        )}
        {stats.skipped > 0 && (
          <span className="ai-graph-stat ai-graph-stat--skipped" title="上游失败导致未执行">
            跳过 {stats.skipped}
          </span>
        )}
        {stats.cancelled > 0 && (
          <span className="ai-graph-stat ai-graph-stat--cancelled">已取消 {stats.cancelled}</span>
        )}
        <div
          className="ai-graph-progress"
          role="progressbar"
          aria-label="任务执行进度"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={stats.progress}
        >
          <div
            className={cn(
              "ai-graph-progress-bar",
              stats.failed > 0 && "ai-graph-progress-bar--failed",
              stats.failed === 0 && stats.cancelled > 0 && "ai-graph-progress-bar--cancelled",
            )}
            style={{ width: `${stats.progress}%` }}
          />
        </div>
        <span className="ai-graph-stat ai-graph-stat--progress">{stats.progress}%</span>
      </div>
    </header>
  );
}

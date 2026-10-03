import { useCallback, useMemo, useRef } from "react";
import { ArrowLeft, Play, RefreshCw, RotateCcw, Square, X } from "lucide-react";
import type { WorkflowDefinition, WorkflowNodeStatus, WorkflowPlanRecord, WorkflowPlanStatus } from "../../types";
import type { WorkflowPanelView } from "../project/main-tabs";
import { cn } from "../../lib/cn";
import { isRovingKey, nextRovingIndex } from "../../lib/roving-index";
import { Button } from "../ui/button";
import { StatusPill } from "../detail/StatusPill";
import { ExpandMainAreaButton } from "./ExpandMainAreaButton";
import { PLAN_STATUS_META, computeWorkflowLayers } from "./workflow-utils";

interface WorkflowPanelHeaderProps {
  plan: WorkflowPlanRecord | null;
  definition: WorkflowDefinition | null;
  planStatus: WorkflowPlanStatus;
  paused: boolean;
  actionPending: boolean;
  statusByNodeId: Map<string, WorkflowNodeStatus>;
  /** 详情态一级视图（画布 / 执行结果），头部切换控件受控于此。 */
  view: WorkflowPanelView;
  onViewChange: (view: WorkflowPanelView) => void;
  onStart: (mode: "full" | "resume") => void;
  onResumeCheckpoint: () => void;
  onCancel: () => void;
  onClose: () => void;
  /** 返回会话工作流列表（两级视图的详情→列表）。 */
  onBackToList?: () => void;
  /** 对最近一次已收尾的运行重新执行验收（验收模型修复/复检结论）。 */
  onReverify: () => void;
  reverifyPending: boolean;
  /** 扩大/还原占满主区（UI-13：切换布局不触发任务重跑）。 */
  onExpandMainArea?: () => void;
  mainAreaExpanded?: boolean;
}

/** 详情态一级视图页签（tablist 方向键导航的索引基础，顺序即渲染顺序）。 */
const VIEW_TABS: { key: WorkflowPanelView; label: string }[] = [
  { key: "canvas", label: "工作流" },
  { key: "result", label: "执行结果" },
];

/**
 * 工作流编排面板两层头部：标题/状态/验收/操作 + 任务统计/整体进度。
 * 操作语义：draft →「确认执行」(full)；failed/cancelled →「从断点继续」(resume，主)
 * +「完整重跑」(full)；completed →「完整重跑」(full)；running →「停止」，
 * 高危写检查点暂停时另显「继续执行」(workflow_run_resume)。
 * 已收尾的计划另配「重新验收」icon 按钮（workflow_run_reverify）：
 * 验收模型修复后补救「未能验收」结论，或对既有结论复检。
 */
export function WorkflowPanelHeader({
  plan,
  definition,
  planStatus,
  paused,
  actionPending,
  statusByNodeId,
  view,
  onViewChange,
  onStart,
  onResumeCheckpoint,
  onCancel,
  onClose,
  onBackToList,
  onReverify,
  reverifyPending,
  onExpandMainArea,
  mainAreaExpanded = false,
}: WorkflowPanelHeaderProps) {
  const statusMeta = PLAN_STATUS_META[planStatus];
  const canStart = planStatus === "draft";
  const canResumeRun = planStatus === "failed" || planStatus === "cancelled";
  const canFullRerun = planStatus === "completed";
  const canCancel = planStatus === "running";
  // 重验收入口：存在已收尾的运行（非 running 的计划 + 最近一次 run 已出验收字段）
  // 才有意义——验收失败/未能验收时用户修复验收模型后在此补救，也可对既有结论复检。
  const canReverify = planStatus !== "running" && planStatus !== "draft" && Boolean(plan?.runs?.[0]);

  // tablist 方向键（roving tabindex + automatic activation，与 ContextNav 同一
  // 模式）：方向键移动焦点即切换视图；非当前视图的页签 tabindex=-1 不占 Tab 序。
  const viewTabRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const activeViewIndex = Math.max(
    0,
    VIEW_TABS.findIndex((tab) => tab.key === view),
  );
  const handleViewTabsKeyDown = useCallback(
    (event: React.KeyboardEvent) => {
      if (!isRovingKey(event.key, "horizontal")) return;
      event.preventDefault();
      const next = nextRovingIndex({
        count: VIEW_TABS.length,
        current: activeViewIndex,
        key: event.key,
        wrap: true,
        orientation: "horizontal",
      });
      if (next === activeViewIndex) return;
      onViewChange(VIEW_TABS[next].key);
      viewTabRefs.current[next]?.focus();
    },
    [activeViewIndex, onViewChange],
  );

  // ── 头部统计：任务数 / 最大并行（最大层宽）/ 状态计数 / 整体进度 ──
  const stats = useMemo(() => {
    const nodes = definition?.nodes ?? [];
    const layers = definition ? computeWorkflowLayers(definition) : [];
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
    // 粗估 token：ASCII 约 4 字符/token，非 ASCII（中文等）按 1 字符/token 的
    // 保守下限（中文实际约 1~1.5 token/字）。仅按任务描述文本估算——不含
    // 系统提示、工具调用与上下文累积，真实消耗通常高一个数量级，
    // 只作启动前的量级参考（标注「估算」）。
    const estimatedTokens = Math.round(
      nodes.reduce((sum, node) => {
        let ascii = 0;
        let nonAscii = 0;
        for (const ch of node.task) {
          if (ch.charCodeAt(0) < 128) ascii += 1;
          else nonAscii += 1;
        }
        return sum + ascii / 4 + nonAscii;
      }, 0),
    );
    return { total, maxParallel, progress, codingNodes, estimatedTokens, ...counts };
  }, [definition, statusByNodeId]);

  // 最近一次运行的验收结论（runs 按 attemptNo 倒序，取首个）。
  // 「运行结果」与「验收结果」是两个独立结论（UI-13）：运行中还没有验收
  // 结论，显示 neutral「验收未开始」（真实字段映射，不虚构）；终态但后端
  // 归一为 unknown 时是「未能验收」。尚未创建过运行（draft）不显示验收组。
  const latestVerdict = useMemo(() => {
    const run = plan?.runs?.[0];
    if (!run) return null;
    if (run.status === "running") {
      return { status: "unknown", label: "验收未开始", reason: null as string | null };
    }
    if (!run.verdictStatus) return null;
    return { status: run.verdictStatus, label: undefined, reason: run.verdictReason ?? null };
  }, [plan]);

  return (
    <header className="ai-workflow-panel-header">
      <div className="ai-workflow-panel-header-top">
        {onBackToList && (
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label="返回工作流列表"
            title="返回工作流列表"
            onClick={onBackToList}
          >
            <ArrowLeft className="h-4 w-4" />
          </Button>
        )}
        <div className="ai-workflow-panel-heading">
          <span className="ai-workflow-panel-title">{plan?.title ?? "工作流"}</span>
          {plan?.summary && (
            <span className="ai-workflow-panel-summary" title={plan.summary}>
              {plan.summary}
            </span>
          )}
        </div>
        {/* 详情态一级视图切换：画布 ↔ 执行结果。最近一次运行已产出结果时
            结果页签带强调标记（运行完成后结果是最值得看的落点）。 */}
        <div
          className="ai-workflow-view-switch"
          role="tablist"
          aria-label="工作流详情视图"
          onKeyDown={handleViewTabsKeyDown}
        >
          {VIEW_TABS.map((tab, index) => (
            <button
              key={tab.key}
              ref={(element) => {
                viewTabRefs.current[index] = element;
              }}
              type="button"
              role="tab"
              aria-selected={view === tab.key}
              tabIndex={view === tab.key ? 0 : -1}
              className={cn(
                "ai-workflow-view-switch-tab",
                view === tab.key && "is-active",
                tab.key === "result" && Boolean(plan?.runs?.[0]?.result) && "has-result",
              )}
              onClick={() => onViewChange(tab.key)}
            >
              {tab.label}
            </button>
          ))}
        </div>
        {/* 两结论独立分区（UI-13）：运行状态与验收结论各带前缀标签，不混为一谈 */}
        <span className="ai-workflow-header-conclusion">
          <span className="ai-workflow-header-pill-label">运行</span>
          <span className={cn("ai-workflow-chip", statusMeta.className)}>{statusMeta.label}</span>
        </span>
        {latestVerdict && (
          <span
            className="ai-workflow-header-conclusion"
            title={latestVerdict.reason ?? undefined}
          >
            <span className="ai-workflow-header-pill-label">验收</span>
            <StatusPill domain="verdict" status={latestVerdict.status} label={latestVerdict.label} />
          </span>
        )}
        {canReverify && (
          <Button
            variant="ghost"
            size="icon-sm"
            className="ai-workflow-header-reverify"
            aria-label="重新验收"
            title={
              latestVerdict?.status === "unknown"
                ? "重新验收：验收模型修复后重跑验收评审（当前未能验收）"
                : "重新验收：对最近一次已结束的运行重跑验收评审"
            }
            onClick={onReverify}
            disabled={reverifyPending || actionPending}
          >
            <RefreshCw className={cn("h-3.5 w-3.5", reverifyPending && "animate-spin")} />
          </Button>
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
        {onExpandMainArea && (
          <ExpandMainAreaButton expanded={mainAreaExpanded} onClick={onExpandMainArea} />
        )}
        <Button variant="ghost" size="icon-sm" aria-label="关闭工作流标签" onClick={onClose}>
          <X className="h-4 w-4" />
        </Button>
      </div>
      <div className="ai-workflow-panel-header-stats">
        <span className="ai-workflow-stat">任务 {stats.total}</span>
        <span className="ai-workflow-stat">并行 {stats.maxParallel}</span>
        {stats.codingNodes > 0 && (
          <span
            className="ai-workflow-stat ai-workflow-stat--failed"
            title="这些节点可能修改文件或执行命令"
          >
            写节点 {stats.codingNodes}
          </span>
        )}
        {stats.estimatedTokens > 0 && (
          <span
            className="ai-workflow-stat"
            title="按任务描述文本粗估输入量级（不含系统提示与工具执行开销，实际消耗通常更高），仅供参考"
          >
            ≈{stats.estimatedTokens.toLocaleString()} tokens（估算）
          </span>
        )}
        {stats.running > 0 && (
          <span className="ai-workflow-stat ai-workflow-stat--running">运行中 {stats.running}</span>
        )}
        {stats.succeeded > 0 && (
          <span className="ai-workflow-stat ai-workflow-stat--succeeded">成功 {stats.succeeded}</span>
        )}
        {stats.failed > 0 && (
          <span className="ai-workflow-stat ai-workflow-stat--failed">失败 {stats.failed}</span>
        )}
        {stats.skipped > 0 && (
          <span className="ai-workflow-stat ai-workflow-stat--skipped" title="上游失败导致未执行">
            跳过 {stats.skipped}
          </span>
        )}
        {stats.cancelled > 0 && (
          <span className="ai-workflow-stat ai-workflow-stat--cancelled">已取消 {stats.cancelled}</span>
        )}
        <div
          className="ai-workflow-progress"
          role="progressbar"
          aria-label="任务执行进度"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={stats.progress}
        >
          <div
            className={cn(
              "ai-workflow-progress-bar",
              stats.failed > 0 && "ai-workflow-progress-bar--failed",
              stats.failed === 0 && stats.cancelled > 0 && "ai-workflow-progress-bar--cancelled",
            )}
            style={{ width: `${stats.progress}%` }}
          />
        </div>
        <span className="ai-workflow-stat ai-workflow-stat--progress">{stats.progress}%</span>
      </div>
    </header>
  );
}

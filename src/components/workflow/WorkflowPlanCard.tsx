import { memo } from "react";
import { Workflow } from "lucide-react";
import { cn } from "../../lib/cn";
import { useWorkspaceStore } from "../../stores/workspace-store";
import { useSessionScope } from "../chat/session-scope";
import { hydrateWorkflowPlan, useWorkflowPlan } from "./workflow-store";
import { Button } from "../ui/button";
import {
  PLAN_STATUS_META,
  normalizePlanStatus,
  parseWorkflowDefinition,
  resultKindBadgeLabel,
  resultKindMeta,
} from "./workflow-utils";

export interface WorkflowPlanCardProps {
  planId: string;
  /** 会话作用域缺失时的回退（工具消息自带的 workspaceId）。 */
  sessionId?: string | null;
  className?: string;
}

/**
 * 消息流内联的工作流计划卡片：submit_workflow 工具消息下方渲染，
 * 状态经 useWorkflowPlan 实时刷新，点击打开全屏工作流面板。
 */
export const WorkflowPlanCard = memo(function WorkflowPlanCard({
  planId,
  sessionId: sessionIdProp,
  className,
}: WorkflowPlanCardProps) {
  const snapshot = useWorkflowPlan(planId);
  const scopedSessionId = useSessionScope();
  const sessionId = scopedSessionId ?? sessionIdProp ?? null;
  const openWorkflowPanel = useWorkspaceStore((state) => state.openWorkflowPanel);

  const plan = snapshot.plan;
  const definition = parseWorkflowDefinition(plan);
  const title = plan?.title || definition?.title || "工作流计划";
  const summary = plan?.summary || definition?.summary || "";
  const nodeCount = definition?.nodes.length ?? plan?.nodeRuns.length ?? 0;
  const statusMeta = plan ? PLAN_STATUS_META[normalizePlanStatus(plan.status)] : null;
  // 最近一次运行已产出执行结果时补结果徽标（run.result 结构化落库后可见）。
  const result = plan?.runs?.[0]?.result ?? null;
  const resultBadge = result
    ? resultKindBadgeLabel(result.resultKind, result.modifiedFiles.length)
    : null;

  const open = () => {
    if (!sessionId) return;
    // 与 useWorkflowPanelController.open 同入口语义：先 hydrate 再打开。
    void hydrateWorkflowPlan(planId);
    openWorkflowPanel(sessionId, planId);
  };

  return (
    <div
      className={cn("ai-workflow-plan-card", className)}
      role="button"
      tabIndex={0}
      aria-label={`查看工作流 ${title}`}
      onClick={open}
      onKeyDown={(event) => {
        if (event.key !== "Enter" && event.key !== " ") return;
        event.preventDefault();
        open();
      }}
    >
      <div className="ai-workflow-plan-card-icon" aria-hidden>
        <Workflow className="h-4 w-4" />
      </div>
      <div className="ai-workflow-plan-card-main">
        <div className="ai-workflow-plan-card-title-row">
          <span className="ai-workflow-plan-card-title" title={title}>
            {title}
          </span>
          {statusMeta && (
            <span className={cn("ai-workflow-chip", statusMeta.className)}>{statusMeta.label}</span>
          )}
          {resultBadge && (
            <span className={cn("ai-workflow-chip", resultKindMeta(result?.resultKind ?? "unknown").className)}>
              {resultBadge}
            </span>
          )}
        </div>
        {summary && <div className="ai-workflow-plan-card-summary">{summary}</div>}
        <div className="ai-workflow-plan-card-footer">
          <span className="ai-workflow-plan-card-meta">
            {nodeCount > 0 ? `${nodeCount} 个节点` : plan ? "" : "计划加载中…"}
          </span>
          <Button
            type="button"
            variant="outline"
            size="sm"
            tabIndex={-1}
            aria-hidden
            className="pointer-events-none h-7"
          >
            查看工作流
          </Button>
        </div>
      </div>
    </div>
  );
});

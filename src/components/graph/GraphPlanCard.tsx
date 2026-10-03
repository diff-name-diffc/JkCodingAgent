import { memo } from "react";
import { Workflow } from "lucide-react";
import { cn } from "../../lib/cn";
import { useWorkspaceStore } from "../../stores/workspace-store";
import { useSessionScope } from "../chat/session-scope";
import { hydrateGraphPlan } from "./graph-store";
import { Button } from "../ui/button";
import { useGraphPlan } from "./graph-store";
import {
  PLAN_STATUS_META,
  normalizePlanStatus,
  parseGraphDefinition,
  resultKindBadgeLabel,
  resultKindMeta,
} from "./graph-utils";

export interface GraphPlanCardProps {
  planId: string;
  /** 会话作用域缺失时的回退（工具消息自带的 workspaceId）。 */
  sessionId?: string | null;
  className?: string;
}

/**
 * 消息流内联的图计划卡片：submit_graph 工具消息下方渲染，
 * 状态经 useGraphPlan 实时刷新，点击打开全屏执行图面板。
 */
export const GraphPlanCard = memo(function GraphPlanCard({
  planId,
  sessionId: sessionIdProp,
  className,
}: GraphPlanCardProps) {
  const snapshot = useGraphPlan(planId);
  const scopedSessionId = useSessionScope();
  const sessionId = scopedSessionId ?? sessionIdProp ?? null;
  const openGraphPanel = useWorkspaceStore((state) => state.openGraphPanel);

  const plan = snapshot.plan;
  const definition = parseGraphDefinition(plan);
  const title = plan?.title || definition?.title || "执行图计划";
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
    // 与 useGraphPanelController.open 同入口语义：先 hydrate 再打开。
    void hydrateGraphPlan(planId);
    openGraphPanel(sessionId, planId);
  };

  return (
    <div
      className={cn("ai-graph-plan-card", className)}
      role="button"
      tabIndex={0}
      aria-label={`查看执行图 ${title}`}
      onClick={open}
      onKeyDown={(event) => {
        if (event.key !== "Enter" && event.key !== " ") return;
        event.preventDefault();
        open();
      }}
    >
      <div className="ai-graph-plan-card-icon" aria-hidden>
        <Workflow className="h-4 w-4" />
      </div>
      <div className="ai-graph-plan-card-main">
        <div className="ai-graph-plan-card-title-row">
          <span className="ai-graph-plan-card-title" title={title}>
            {title}
          </span>
          {statusMeta && (
            <span className={cn("ai-graph-chip", statusMeta.className)}>{statusMeta.label}</span>
          )}
          {resultBadge && (
            <span className={cn("ai-graph-chip", resultKindMeta(result?.resultKind ?? "unknown").className)}>
              {resultBadge}
            </span>
          )}
        </div>
        {summary && <div className="ai-graph-plan-card-summary">{summary}</div>}
        <div className="ai-graph-plan-card-footer">
          <span className="ai-graph-plan-card-meta">
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
            查看执行图
          </Button>
        </div>
      </div>
    </div>
  );
});

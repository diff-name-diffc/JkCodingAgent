import { memo } from "react";
import { Workflow } from "lucide-react";
import { cn } from "../../lib/cn";
import { useUIStore } from "../../stores/ui-store";
import { Button } from "../ui/button";
import { useGraphPlan } from "./graph-store";
import {
  PLAN_STATUS_META,
  normalizePlanStatus,
  parseGraphDefinition,
} from "./graph-utils";

export interface GraphPlanCardProps {
  planId: string;
  className?: string;
}

/**
 * 消息流内联的图计划卡片：submit_graph 工具消息下方渲染，
 * 状态经 useGraphPlan 实时刷新，点击打开全屏执行图面板。
 */
export const GraphPlanCard = memo(function GraphPlanCard({
  planId,
  className,
}: GraphPlanCardProps) {
  const snapshot = useGraphPlan(planId);
  const setGraphPanelPlanId = useUIStore((state) => state.setGraphPanelPlanId);

  const plan = snapshot.plan;
  const definition = parseGraphDefinition(plan);
  const title = plan?.title || definition?.title || "执行图计划";
  const summary = plan?.summary || definition?.summary || "";
  const nodeCount = definition?.nodes.length ?? plan?.nodeRuns.length ?? 0;
  const statusMeta = plan ? PLAN_STATUS_META[normalizePlanStatus(plan.status)] : null;

  return (
    <div className={cn("ai-graph-plan-card", className)}>
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
            className="h-7"
            onClick={() => setGraphPanelPlanId(planId)}
          >
            查看执行图
          </Button>
        </div>
      </div>
    </div>
  );
});

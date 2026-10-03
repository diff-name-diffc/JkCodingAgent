import { memo, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Network, X } from "lucide-react";
import type { GraphPlanListItem, GraphPlanUpdatedPayload, GraphRunEventPayload } from "../../types";
import { formatRelativeTime } from "../../utils";
import { cn } from "../../lib/cn";
import { Button } from "../ui/button";
import { EmptyState } from "../settings/EmptyState";
import { resolveStatusMeta } from "../detail/status-meta";
import { hydrateGraphPlan } from "./graph-store";
import { PLAN_STATUS_META, resultKindBadgeLabel, resultKindMeta } from "./graph-utils";
import { ExpandMainAreaButton } from "./ExpandMainAreaButton";
import { useWorkspaceStore } from "../../stores/workspace-store";

export interface GraphPlanListViewProps {
  /** 归属会话：列表数据与意图通道都按会话隔离。 */
  sessionId: string;
  /** 工作区与编辑 pane 当前是否可见（保活门控：重新可见时兜底重查，与
   * GraphPanel / BrowserPanel 的 active 语义一致）。 */
  active: boolean;
  onClose: () => void;
  onExpandMainArea?: () => void;
  mainAreaExpanded?: boolean;
}

/** run 生命周期事件才影响列表行状态（节点增量不改变 plan 摘要）。 */
const RUN_LIFECYCLE_EVENTS = new Set([
  "runStarted",
  "runFinished",
  "runFailed",
  "runCancelled",
]);

/**
 * 会话执行图列表（两级视图的列表态）：展示本会话全部图计划摘要，点击行经
 * 意图通道切入该图详情（GraphPanel，消息流 GraphPlanCard 同一入口语义）。
 * 异步数据走 React Query：graph-plan-updated 与 run 生命周期事件失效重查。
 */
export function GraphPlanListView({
  sessionId,
  active,
  onClose,
  onExpandMainArea,
  mainAreaExpanded = false,
}: GraphPlanListViewProps) {
  const queryClient = useQueryClient();
  const openGraphPanel = useWorkspaceStore((state) => state.openGraphPanel);

  const { data: plans, isPending, isError } = useQuery({
    queryKey: ["graph-plan-list", sessionId],
    queryFn: () => invoke<GraphPlanListItem[]>("graph_plan_list_for_session", {
      workspaceId: sessionId,
    }),
  });

  useEffect(() => {
    // 计划登记/定义更新/状态流转都会触发失效；run 生命周期事件改变行内
    // 最近运行摘要（状态、验收），节点级增量事件与列表无关。
    const stopPlanUpdated = listen<GraphPlanUpdatedPayload>("graph-plan-updated", ({ payload }) => {
      if (payload.workspaceId === sessionId) {
        void queryClient.invalidateQueries({ queryKey: ["graph-plan-list", sessionId] });
      }
    });
    const stopRunEvent = listen<GraphRunEventPayload>("graph-run-event", ({ payload }) => {
      if (payload.workspaceId === sessionId && RUN_LIFECYCLE_EVENTS.has(payload.event)) {
        void queryClient.invalidateQueries({ queryKey: ["graph-plan-list", sessionId] });
      }
    });
    return () => {
      void stopPlanUpdated.then((stop) => stop());
      void stopRunEvent.then((stop) => stop());
    };
  }, [sessionId, queryClient]);

  // 保活切回兜底：面板重新可见时失效重查（隐藏期间事件监听持续保缓存
  // 新鲜，这里防御事件缺口；仅响应 false→true 跳变，首挂载交给 useQuery）。
  const prevActiveRef = useRef(active);
  useEffect(() => {
    const becameActive = active && !prevActiveRef.current;
    prevActiveRef.current = active;
    if (!becameActive) return;
    void queryClient.invalidateQueries({ queryKey: ["graph-plan-list", sessionId] });
  }, [active, sessionId, queryClient]);

  const openPlan = (planId: string, view: "canvas" | "result") => {
    // 预取详情快照后走意图通道切标签（与 GraphPlanCard 同构）；
    // view="result" 直达详情态的执行结果视图（列表行「结果」入口）。
    void hydrateGraphPlan(planId);
    openGraphPanel(sessionId, planId, view);
  };

  return (
    <div className="ai-graph-panel" role="region" aria-label="会话执行图列表">
      <header className="ai-graph-panel-header">
        <div className="ai-graph-panel-header-top">
          <div className="ai-graph-panel-heading">
            <span className="ai-graph-panel-title">执行图</span>
            <span className="ai-graph-panel-summary">本会话全部图计划（{plans?.length ?? 0}）</span>
          </div>
          {onExpandMainArea && (
            <ExpandMainAreaButton expanded={mainAreaExpanded} onClick={onExpandMainArea} />
          )}
          <Button variant="ghost" size="icon-sm" aria-label="关闭执行图标签" onClick={onClose}>
            <X className="h-4 w-4" />
          </Button>
        </div>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto p-3">
        {isPending && <p className="px-2 py-8 text-center text-sm text-muted-foreground">加载中…</p>}
        {isError && (
          <p className="px-2 py-8 text-center text-sm text-danger">执行图列表加载失败，请重试。</p>
        )}
        {plans && plans.length === 0 && (
          <EmptyState icon={Network} title="本会话还没有执行图" />
        )}
        {plans && plans.length > 0 && (
          <ul className="flex flex-col gap-2">
            {plans.map((item) => (
              <GraphPlanRow key={item.id} item={item} onOpen={openPlan} />
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

const GraphPlanRow = memo(function GraphPlanRow({
  item,
  onOpen,
}: {
  item: GraphPlanListItem;
  onOpen: (planId: string, view: "canvas" | "result") => void;
}) {
  const statusMeta = PLAN_STATUS_META[item.status] ?? PLAN_STATUS_META.draft;
  // 验收结论与详情态同源（detail/status-meta.ts 的 verdict 域），空串不展示。
  const verdict = item.latestRun?.verdictStatus
    ? resolveStatusMeta("verdict", item.latestRun.verdictStatus).label
    : null;
  const resultBadge = item.latestRun
    ? resultKindBadgeLabel(item.latestRun.resultKind, item.latestRun.modifiedFileCount)
    : null;
  return (
    <li className="flex items-stretch gap-1.5">
      <button
        type="button"
        onClick={() => onOpen(item.id, "canvas")}
        className="flex min-w-0 flex-1 flex-col gap-1.5 rounded-md border border-border/60 bg-card px-3 py-2.5 text-left transition-colors hover:bg-sidebar-hover focus-visible:outline-2 focus-visible:outline-accent"
        title={item.summary || item.title}
      >
        <span className="flex items-center gap-2">
          <span className={cn("ai-graph-chip", statusMeta.className)}>{statusMeta.label}</span>
          {resultBadge && (
            <span
              className={cn(
                "ai-graph-chip",
                resultKindMeta(item.latestRun?.resultKind ?? "unknown").className,
              )}
            >
              {resultBadge}
            </span>
          )}
          <span className="min-w-0 flex-1 truncate text-sm font-medium text-foreground">
            {item.title}
          </span>
          <span className="shrink-0 text-xs text-muted-foreground">
            {formatRelativeTime(new Date(item.updatedAt).toISOString())}
          </span>
        </span>
        <span className="flex items-center gap-3 text-xs text-muted-foreground">
          <span>{item.nodeCount} 个节点</span>
          {item.latestRun && (
            <span>
              第 {item.latestRun.attemptNo} 次运行
              {verdict ? ` · ${verdict}` : ""}
            </span>
          )}
          {item.summary && <span className="min-w-0 flex-1 truncate">{item.summary}</span>}
        </span>
      </button>
      {resultBadge && item.latestRun && (
        <button
          type="button"
          className="ai-graph-list-result-btn"
          onClick={() => onOpen(item.id, "result")}
          title={
            item.latestRun.conclusionPreview
              ? `查看${resultBadge}：${item.latestRun.conclusionPreview}`
              : `查看${resultBadge}`
          }
        >
          结果
        </button>
      )}
    </li>
  );
});

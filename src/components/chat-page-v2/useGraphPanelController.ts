import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import type { GraphPlanRecord, GraphPlanUpdatedPayload } from "../../types";
import { useWorkspaceStore } from "../../stores/workspace-store";
import { hydrateGraphPlan } from "../graph/graph-store";

export function useGraphPanelController(
  activeSessionId: string | null,
  isPlainChat: boolean,
  currentSessionIdRef: React.RefObject<string | null>,
) {
  // 执行图打开意图绑定 sessionId（UI-08）：多项目保活挂载不再共享 planId。
  // UI-13 起意图由 useGraphTabSync 消费为主区标签，标签是渲染真值；
  // 本控制器只保留意图开/关、最近计划入口与截断后的刷新。
  const openGraphPanel = useWorkspaceStore((state) => state.openGraphPanel);
  const closeGraphPanel = useWorkspaceStore((state) => state.closeGraphPanel);
  const [latestPlanId, setLatestPlanId] = useState<string | null>(null);
  // 截断（regenerate / 编辑重发）会删除被删轮次的图计划，refreshLatestPlan
  // 递增该 tick 触发重新查询，避免「最近计划」入口指向已删除的计划。
  const [refreshTick, setRefreshTick] = useState(0);

  useEffect(() => {
    if (isPlainChat || !activeSessionId) {
      setLatestPlanId(null);
      return;
    }
    let cancelled = false;
    invoke<GraphPlanRecord | null>("graph_plan_latest_for_session", {
      workspaceId: activeSessionId,
    })
      .then((plan) => {
        if (!cancelled) setLatestPlanId(plan?.id ?? null);
      })
      .catch((error) => {
        if (!cancelled) {
          setLatestPlanId(null);
          console.error("查询最近图计划失败:", error);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [activeSessionId, isPlainChat, refreshTick]);

  useEffect(() => {
    if (isPlainChat) return;
    const unlisten = listen<GraphPlanUpdatedPayload>("graph-plan-updated", ({ payload }) => {
      if (payload.workspaceId === currentSessionIdRef.current) setLatestPlanId(payload.planId);
    });
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, [currentSessionIdRef, isPlainChat]);

  const open = useCallback(() => {
    if (!latestPlanId || !activeSessionId) return;
    void hydrateGraphPlan(latestPlanId);
    openGraphPanel(activeSessionId, latestPlanId);
  }, [latestPlanId, activeSessionId, openGraphPanel]);

  return {
    latestPlanId,
    open,
    close: useCallback(() => closeGraphPanel(activeSessionId ?? undefined), [closeGraphPanel, activeSessionId]),
    refreshLatestPlan: useCallback(() => setRefreshTick((tick) => tick + 1), []),
  };
}

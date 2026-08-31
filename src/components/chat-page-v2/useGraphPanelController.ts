import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import type { GraphPlanRecord, GraphPlanUpdatedPayload } from "../../types";
import { useUIStore } from "../../stores/ui-store";
import { hydrateGraphPlan } from "../graph/graph-store";

export function useGraphPanelController(
  activeSessionId: string | null,
  isPlainChat: boolean,
  currentSessionIdRef: React.RefObject<string | null>,
) {
  const planId = useUIStore((state) => state.graphPanelPlanId);
  const setPlanId = useUIStore((state) => state.setGraphPanelPlanId);
  const [latestPlanId, setLatestPlanId] = useState<string | null>(null);

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
  }, [activeSessionId, isPlainChat]);

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
    if (!latestPlanId) return;
    void hydrateGraphPlan(latestPlanId);
    setPlanId(latestPlanId);
  }, [latestPlanId, setPlanId]);

  return {
    planId,
    latestPlanId,
    open,
    close: useCallback(() => setPlanId(null), [setPlanId]),
  };
}

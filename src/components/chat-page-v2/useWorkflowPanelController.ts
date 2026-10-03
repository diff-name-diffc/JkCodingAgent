import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import type { WorkflowPlanRecord, WorkflowPlanUpdatedPayload } from "../../types";
import { useWorkspaceStore } from "../../stores/workspace-store";

export function useWorkflowPanelController(
  activeSessionId: string | null,
  isPlainChat: boolean,
  currentSessionIdRef: React.RefObject<string | null>,
) {
  // 工作流打开意图绑定 sessionId（UI-08）：多项目保活挂载不再共享 planId。
  // UI-13 起意图由 useWorkflowTabSync 消费为主区标签，标签是渲染真值；
  // 本控制器只保留意图开/关、最近计划入口与截断后的刷新。
  const openWorkflowPanel = useWorkspaceStore((state) => state.openWorkflowPanel);
  const closeWorkflowPanel = useWorkspaceStore((state) => state.closeWorkflowPanel);
  const [latestPlanId, setLatestPlanId] = useState<string | null>(null);
  // 截断（regenerate / 编辑重发）会删除被删轮次的工作流计划，refreshLatestPlan
  // 递增该 tick 触发重新查询，避免「最近计划」入口指向已删除的计划。
  const [refreshTick, setRefreshTick] = useState(0);

  useEffect(() => {
    // 会话切换/截断重查前先清空旧值：查询返回前的窗口期按钮可用性不得
    // 沿用旧会话的最近计划（否则可对新会话误开工作流标签）。
    setLatestPlanId(null);
    if (isPlainChat || !activeSessionId) {
      return;
    }
    let cancelled = false;
    invoke<WorkflowPlanRecord | null>("workflow_plan_latest_for_session", {
      workspaceId: activeSessionId,
    })
      .then((plan) => {
        if (!cancelled) setLatestPlanId(plan?.id ?? null);
      })
      .catch((error) => {
        if (!cancelled) {
          setLatestPlanId(null);
          console.error("查询最近工作流计划失败:", error);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [activeSessionId, isPlainChat, refreshTick]);

  useEffect(() => {
    if (isPlainChat) return;
    const unlisten = listen<WorkflowPlanUpdatedPayload>("workflow-plan-updated", ({ payload }) => {
      if (payload.workspaceId === currentSessionIdRef.current) setLatestPlanId(payload.planId);
    });
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, [currentSessionIdRef, isPlainChat]);

  const open = useCallback(() => {
    // 打开的是会话工作流列表（两级视图的列表态）；latestPlanId 仅作可用性门槛
    // （会话从未提交工作流则按钮不可用）。
    if (!latestPlanId || !activeSessionId) return;
    openWorkflowPanel(activeSessionId, null);
  }, [latestPlanId, activeSessionId, openWorkflowPanel]);

  return {
    latestPlanId,
    open,
    close: useCallback(
      () => closeWorkflowPanel(activeSessionId ?? undefined),
      [closeWorkflowPanel, activeSessionId],
    ),
    refreshLatestPlan: useCallback(() => setRefreshTick((tick) => tick + 1), []),
  };
}

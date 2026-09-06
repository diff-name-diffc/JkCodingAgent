import { useCallback, useEffect } from "react";
import { useWorkspaceStore } from "../stores/workspace-store";
import type { EditorTab, EditorTabsState } from "../components/project/main-tabs";

export type GraphTab = Extract<EditorTab, { kind: "graph" }>;

interface UseGraphTabSyncOptions {
  activeSessionId: string | null;
  editorTabs: EditorTabsState;
  onOpenGraphTab: (planId: string, sessionId: string) => void;
  onCloseGraphTab: (tabId: string) => void;
}

/**
 * 执行图「打开意图 → 主区标签」同步（UI-13）。
 *
 * workspace-store.graphPanel 保留为跨组件意图通道（GraphPlanCard / 头部按钮
 * 的深层调用零改动）；标签是渲染真值。双向 effect 语义单调，避免竞态：
 * - 意图指向当前活动会话 → 打开/激活标签（openGraphTab 幂等）；
 * - 意图被清空（truncate / regenerate 链路调用 closeGraphPanel）→ 关该会话图标签；
 * - 用户关标签 → 仅当意图与会话+计划完全匹配时清除（防跨会话误清）。
 *
 * 多项目保活安全：每个 ProjectPage 实例持有自己的 editorTabs，
 * 意图 sessionId 不匹配时两个 effect 均不动作。
 */
export function useGraphTabSync({
  activeSessionId,
  editorTabs,
  onOpenGraphTab,
  onCloseGraphTab,
}: UseGraphTabSyncOptions) {
  const graphPanel = useWorkspaceStore((state) => state.graphPanel);
  const closeGraphPanel = useWorkspaceStore((state) => state.closeGraphPanel);

  useEffect(() => {
    if (!graphPanel || !activeSessionId) return;
    if (graphPanel.sessionId !== activeSessionId) return;
    onOpenGraphTab(graphPanel.planId, graphPanel.sessionId);
  }, [graphPanel, activeSessionId, onOpenGraphTab]);

  useEffect(() => {
    if (graphPanel || !activeSessionId) return;
    const tab = editorTabs.tabs.find(
      (candidate): candidate is GraphTab =>
        candidate.kind === "graph" && candidate.sessionId === activeSessionId,
    );
    if (tab) onCloseGraphTab(tab.id);
  }, [graphPanel, activeSessionId, editorTabs, onCloseGraphTab]);

  const closeGraphTab = useCallback(
    (tab: GraphTab) => {
      onCloseGraphTab(tab.id);
      const intent = useWorkspaceStore.getState().graphPanel;
      if (intent && intent.sessionId === tab.sessionId && intent.planId === tab.planId) {
        closeGraphPanel(tab.sessionId);
      }
    },
    [onCloseGraphTab, closeGraphPanel],
  );

  return { closeGraphTab };
}

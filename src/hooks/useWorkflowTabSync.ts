import { useCallback, useEffect } from "react";
import { useWorkspaceStore } from "../stores/workspace-store";
import type {
  EditorTab,
  EditorTabsState,
  WorkflowPanelView,
} from "../components/project/main-tabs";

export type WorkflowTab = Extract<EditorTab, { kind: "workflow" }>;

interface UseWorkflowTabSyncOptions {
  activeSessionId: string | null;
  editorTabs: EditorTabsState;
  onOpenWorkflowTab: (sessionId: string, planId: string | null, view: WorkflowPanelView) => void;
  onCloseWorkflowTab: (tabId: string) => void;
  /** 每次用户点开工作流的序号。用来在放不下双栏时让出会话栏，且不重复抢回。 */
  onWorkflowOpenRequest?: (openSeq: number) => void;
}

/**
 * 工作流「打开意图 → 主区标签」同步（UI-13）。
 *
 * workspace-store.workflowPanel 保留为跨组件意图通道（WorkflowPlanCard / 头部按钮
 * 的深层调用零改动）；标签是渲染真值。双向 effect 语义单调，避免竞态：
 * - 意图指向当前活动会话 → 打开/激活标签（openWorkflowTab 幂等）；
 * - 意图被清空（truncate / regenerate 链路调用 closeWorkflowPanel）→ 关该会话工作流标签；
 * - 用户关标签 → 仅当意图与会话+计划完全匹配时清除（防跨会话误清）。
 *
 * 多项目保活安全：每个 ProjectPage 实例持有自己的 editorTabs，
 * 意图 sessionId 不匹配时两个 effect 均不动作。
 */
export function useWorkflowTabSync({
  activeSessionId,
  editorTabs,
  onOpenWorkflowTab,
  onCloseWorkflowTab,
  onWorkflowOpenRequest,
}: UseWorkflowTabSyncOptions) {
  const workflowPanel = useWorkspaceStore((state) => state.workflowPanel);
  const closeWorkflowPanel = useWorkspaceStore((state) => state.closeWorkflowPanel);

  useEffect(() => {
    if (!workflowPanel || !activeSessionId) return;
    if (workflowPanel.sessionId !== activeSessionId) return;
    onOpenWorkflowTab(workflowPanel.sessionId, workflowPanel.planId, workflowPanel.view);
    onWorkflowOpenRequest?.(workflowPanel.openSeq);
  }, [workflowPanel, activeSessionId, onOpenWorkflowTab, onWorkflowOpenRequest]);

  useEffect(() => {
    if (workflowPanel || !activeSessionId) return;
    const tab = editorTabs.tabs.find(
      (candidate): candidate is WorkflowTab =>
        candidate.kind === "workflow" && candidate.sessionId === activeSessionId,
    );
    if (tab) onCloseWorkflowTab(tab.id);
  }, [workflowPanel, activeSessionId, editorTabs, onCloseWorkflowTab]);

  const closeWorkflowTab = useCallback(
    (tab: WorkflowTab) => {
      onCloseWorkflowTab(tab.id);
      const intent = useWorkspaceStore.getState().workflowPanel;
      if (intent && intent.sessionId === tab.sessionId && intent.planId === tab.planId) {
        closeWorkflowPanel(tab.sessionId);
      }
    },
    [onCloseWorkflowTab, closeWorkflowPanel],
  );

  return { closeWorkflowTab };
}

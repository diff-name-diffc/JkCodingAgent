import { create } from "zustand";
import { persist } from "zustand/middleware";
import { DEFAULT_WORKFLOW_PANEL_VIEW, type WorkflowPanelView } from "../components/project/main-tabs";
import {
  DEFAULT_WORKSPACE_PREFS,
  sanitizeWorkspacePrefs,
  type WorkspacePrefs,
} from "../components/project/workspace-prefs";

interface WorkspaceState {
  /** 每工作区布局偏好（持久化）。窄窗临时适配不得写入这里。 */
  prefsByWorkspace: Record<string, WorkspacePrefs>;
  /**
   * 每会话临时详情（不持久化）：工作流面板归属。
   * 绑定 sessionId 后，多项目保活挂载不再共享同一个 planId（UI-08 串台修复）。
   * UI-13 起作为「打开工作流标签」的意图通道：tab 是渲染真值，store 只是意图。
   * planId 为 null 表示列表态（会话全部工作流列表），非 null 直达该工作流详情；
   * view 指定详情态初始一级视图（画布 / 执行结果，列表行「结果」入口用）。
   */
  workflowPanel: {
    sessionId: string;
    planId: string | null;
    view: WorkflowPanelView;
    openSeq: number;
  } | null;
  /**
   * 每会话工作流视图记忆（不持久化，UI-13/UI-14）：工作流标签关闭再打开、
   * 详情返回时保留选中节点、视口、共享状态展开态与手动布局覆盖。
   */
  workflowViewBySession: Record<string, WorkflowViewMemory>;
  setPrefs: (workspaceId: string, patch: Partial<WorkspacePrefs>) => void;
  openWorkflowPanel: (sessionId: string, planId: string | null, view?: WorkflowPanelView) => void;
  closeWorkflowPanel: (sessionId?: string) => void;
  setWorkflowView: (sessionId: string, patch: Partial<WorkflowViewMemory>) => void;
  clearWorkflowView: (sessionId: string) => void;
}

/** 工作流视图记忆（每会话临时层；缺失字段由读取方回退默认值）。 */
export interface WorkflowViewMemory {
  planId: string;
  selectedNodeId: string | null;
  viewport: { x: number; y: number; zoom: number } | null;
  stateOpen: boolean;
  dragOverrides: Record<string, { x: number; y: number }>;
}

export const useWorkspaceStore = create<WorkspaceState>()(
  persist(
    (set) => ({
      prefsByWorkspace: {},
      workflowPanel: null,
      workflowViewBySession: {},
      setPrefs: (workspaceId, patch) =>
        set((state) => {
          const current = sanitizeWorkspacePrefs(state.prefsByWorkspace[workspaceId]);
          return {
            prefsByWorkspace: {
              ...state.prefsByWorkspace,
              [workspaceId]: sanitizeWorkspacePrefs({ ...current, ...patch }),
            },
          };
        }),
      openWorkflowPanel: (sessionId, planId, view = DEFAULT_WORKFLOW_PANEL_VIEW) =>
        set((state) => ({
          workflowPanel: {
            sessionId,
            planId,
            view,
            // 同一次点击的序号。重复点同一个工作流也要递增，窄窗才能再次把编辑区让出来。
            openSeq: (state.workflowPanel?.openSeq ?? 0) + 1,
          },
        })),
      closeWorkflowPanel: (sessionId) =>
        set((state) =>
          !sessionId || state.workflowPanel?.sessionId === sessionId
            ? { workflowPanel: null }
            : {},
        ),
      setWorkflowView: (sessionId, patch) =>
        set((state) => ({
          workflowViewBySession: {
            ...state.workflowViewBySession,
            [sessionId]: { ...state.workflowViewBySession[sessionId], ...patch } as WorkflowViewMemory,
          },
        })),
      clearWorkflowView: (sessionId) =>
        set((state) => {
          if (!(sessionId in state.workflowViewBySession)) return {};
          const next = { ...state.workflowViewBySession };
          delete next[sessionId];
          return { workflowViewBySession: next };
        }),
    }),
    {
      name: "jkcodingagent.workspace.v1",
      partialize: (state) => ({ prefsByWorkspace: state.prefsByWorkspace }),
      // 旧版本/手改 localStorage 的非法值在合并时统一校验回退。
      merge: (persisted, current) => {
        const raw = (persisted as Partial<WorkspaceState> | undefined)?.prefsByWorkspace ?? {};
        const cleaned: Record<string, WorkspacePrefs> = {};
        for (const [id, value] of Object.entries(raw)) {
          cleaned[id] = sanitizeWorkspacePrefs(value);
        }
        return {
          ...current,
          prefsByWorkspace: cleaned,
          workflowPanel: null,
          workflowViewBySession: {},
        };
      },
    },
  ),
);

/** 读取某工作区偏好（缺省返回默认值，组件侧无需判空）。 */
export function selectWorkspacePrefs(workspaceId: string) {
  return (state: WorkspaceState): WorkspacePrefs =>
    state.prefsByWorkspace[workspaceId] ?? DEFAULT_WORKSPACE_PREFS;
}

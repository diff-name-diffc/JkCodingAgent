import { create } from "zustand";
import { persist } from "zustand/middleware";
import {
  DEFAULT_WORKSPACE_PREFS,
  sanitizeWorkspacePrefs,
  type WorkspacePrefs,
} from "../components/project/workspace-prefs";

interface WorkspaceState {
  /** 每工作区布局偏好（持久化）。窄窗临时适配不得写入这里。 */
  prefsByWorkspace: Record<string, WorkspacePrefs>;
  /**
   * 每会话临时详情（不持久化）：执行图面板归属。
   * 绑定 sessionId 后，多项目保活挂载不再共享同一个 planId（UI-08 串台修复）。
   * UI-13 起作为「打开执行图标签」的意图通道：tab 是渲染真值，store 只是意图。
   */
  graphPanel: { sessionId: string; planId: string } | null;
  /**
   * 每会话执行图视图记忆（不持久化，UI-13/UI-14）：图标签关闭再打开、
   * 详情返回时保留选中节点、视口、共享状态展开态与手动布局覆盖。
   */
  graphViewBySession: Record<string, GraphViewMemory>;
  setPrefs: (workspaceId: string, patch: Partial<WorkspacePrefs>) => void;
  openGraphPanel: (sessionId: string, planId: string) => void;
  closeGraphPanel: (sessionId?: string) => void;
  setGraphView: (sessionId: string, patch: Partial<GraphViewMemory>) => void;
  clearGraphView: (sessionId: string) => void;
}

/** 执行图视图记忆（每会话临时层；缺失字段由读取方回退默认值）。 */
export interface GraphViewMemory {
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
      graphPanel: null,
      graphViewBySession: {},
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
      openGraphPanel: (sessionId, planId) => set({ graphPanel: { sessionId, planId } }),
      closeGraphPanel: (sessionId) =>
        set((state) =>
          !sessionId || state.graphPanel?.sessionId === sessionId
            ? { graphPanel: null }
            : {},
        ),
      setGraphView: (sessionId, patch) =>
        set((state) => ({
          graphViewBySession: {
            ...state.graphViewBySession,
            [sessionId]: { ...state.graphViewBySession[sessionId], ...patch } as GraphViewMemory,
          },
        })),
      clearGraphView: (sessionId) =>
        set((state) => {
          if (!(sessionId in state.graphViewBySession)) return {};
          const next = { ...state.graphViewBySession };
          delete next[sessionId];
          return { graphViewBySession: next };
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
          graphPanel: null,
          graphViewBySession: {},
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

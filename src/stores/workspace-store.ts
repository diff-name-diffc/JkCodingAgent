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
   */
  graphPanel: { sessionId: string; planId: string } | null;
  setPrefs: (workspaceId: string, patch: Partial<WorkspacePrefs>) => void;
  openGraphPanel: (sessionId: string, planId: string) => void;
  closeGraphPanel: (sessionId?: string) => void;
}

export const useWorkspaceStore = create<WorkspaceState>()(
  persist(
    (set) => ({
      prefsByWorkspace: {},
      graphPanel: null,
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
        return { ...current, prefsByWorkspace: cleaned, graphPanel: null };
      },
    },
  ),
);

/** 读取某工作区偏好（缺省返回默认值，组件侧无需判空）。 */
export function selectWorkspacePrefs(workspaceId: string) {
  return (state: WorkspaceState): WorkspacePrefs =>
    state.prefsByWorkspace[workspaceId] ?? DEFAULT_WORKSPACE_PREFS;
}

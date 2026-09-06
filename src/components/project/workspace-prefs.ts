/**
 * 工作区布局偏好（UI-08）：每工作区一份，持久化；非法旧值在读取时校验回退。
 * 数值区间与 workspace-budget.ts 的 chrome 常量保持一致（终端高度经
 * TERMINAL_HEIGHT_LIMITS 引用同一出处，UI-19）。
 */
import { TERMINAL_HEIGHT_LIMITS } from "./workspace-budget";

export type ContextTab = "sessions" | "files" | "changes" | "history";

export interface WorkspacePrefs {
  contextNavWidth: number;
  contextTab: ContextTab;
  sessionSidebarCollapsed: boolean;
  rightPanelWidth: number;
  terminalHeight: number;
  editorPaneRatio: number;
}

export const DEFAULT_WORKSPACE_PREFS: WorkspacePrefs = {
  contextNavWidth: 248,
  contextTab: "sessions",
  sessionSidebarCollapsed: false,
  rightPanelWidth: 280,
  terminalHeight: 240,
  editorPaneRatio: 0.5,
};

const CONTEXT_TABS: ContextTab[] = ["sessions", "files", "changes", "history"];

function num(value: unknown, fallback: number, min: number, max: number): number {
  if (typeof value !== "number" || !Number.isFinite(value)) return fallback;
  return Math.min(max, Math.max(min, value));
}

/**
 * 校验持久化/外部传入的偏好：类型错误、越界、未知页签一律回退默认。
 * 纯函数，供 store 读取与 rehydrate 合并共用。
 */
export function sanitizeWorkspacePrefs(raw: unknown): WorkspacePrefs {
  const source = (raw ?? {}) as Partial<Record<keyof WorkspacePrefs, unknown>>;
  return {
    contextNavWidth: num(source.contextNavWidth, DEFAULT_WORKSPACE_PREFS.contextNavWidth, 216, 320),
    contextTab: CONTEXT_TABS.includes(source.contextTab as ContextTab)
      ? (source.contextTab as ContextTab)
      : DEFAULT_WORKSPACE_PREFS.contextTab,
    sessionSidebarCollapsed:
      typeof source.sessionSidebarCollapsed === "boolean"
        ? source.sessionSidebarCollapsed
        : DEFAULT_WORKSPACE_PREFS.sessionSidebarCollapsed,
    rightPanelWidth: num(source.rightPanelWidth, DEFAULT_WORKSPACE_PREFS.rightPanelWidth, 180, 600),
    terminalHeight: num(
      source.terminalHeight,
      DEFAULT_WORKSPACE_PREFS.terminalHeight,
      TERMINAL_HEIGHT_LIMITS.min,
      TERMINAL_HEIGHT_LIMITS.max,
    ),
    editorPaneRatio: num(source.editorPaneRatio, DEFAULT_WORKSPACE_PREFS.editorPaneRatio, 0, 1),
  };
}

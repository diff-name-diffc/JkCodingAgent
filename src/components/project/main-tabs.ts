/**
 * 主区编辑pane标签纯 reducer（UI-09）：文件标签与 Git diff 统一为同一
 * 标签体系（diff 不再是与文件互斥的独立槽），关闭/回退语义集中在此便于单测。
 * 组件侧（useProjectPanels）只做状态持有与副作用编排。
 */
import type { OpenDiff } from "../../hooks/projectPanelsFileState";

export type { OpenDiff };

export type EditorTab =
  | { id: string; kind: "file"; path: string; name: string }
  | { id: string; kind: "diff"; diff: OpenDiff }
  /** 执行图工作视图（UI-13）：id 稳定于 planId——切视图不触发新运行。 */
  | { id: string; kind: "graph"; planId: string; sessionId: string };

export interface EditorTabsState {
  tabs: EditorTab[];
  activeTabId: string | null;
}

export const EMPTY_EDITOR_TABS: EditorTabsState = { tabs: [], activeTabId: null };

/** 文件标签 id 稳定于路径，diff 标签 id 稳定于其判别内容。 */
export function fileTabId(path: string): string {
  return `file:${path}`;
}

export function diffTabId(diff: OpenDiff): string {
  if (diff.kind === "file") return `diff:file:${diff.filePath}:${diff.staged ? 1 : 0}`;
  if (diff.kind === "commit-file") return `diff:commit-file:${diff.hash}:${diff.filePath}`;
  return `diff:commit:${diff.hash}`;
}

export function sameDiff(a: OpenDiff, b: OpenDiff): boolean {
  return diffTabId(a) === diffTabId(b);
}

export function graphTabId(planId: string): string {
  return `graph:${planId}`;
}

/** 打开/激活文件标签（已存在则仅激活）。 */
export function openFileTab(state: EditorTabsState, path: string, name: string): EditorTabsState {
  const id = fileTabId(path);
  const existing = state.tabs.find((tab) => tab.id === id);
  const tabs = existing
    ? state.tabs.map((tab) =>
        tab.id === id ? { ...tab, name } : tab,
      )
    : [...state.tabs, { id, kind: "file" as const, path, name }];
  return { tabs, activeTabId: id };
}

/** 打开/激活 diff 标签（同判别内容仅激活；不同 diff 新建标签）。 */
export function openDiffTab(state: EditorTabsState, diff: OpenDiff): EditorTabsState {
  const id = diffTabId(diff);
  const exists = state.tabs.some((tab) => tab.id === id);
  const tabs = exists
    ? state.tabs.map((tab) => (tab.id === id ? { ...tab, diff } : tab))
    : [...state.tabs, { id, kind: "diff" as const, diff }];
  return { tabs, activeTabId: id };
}

export function selectTab(state: EditorTabsState, tabId: string): EditorTabsState {
  if (!state.tabs.some((tab) => tab.id === tabId)) return state;
  return { ...state, activeTabId: tabId };
}

/**
 * 打开/激活执行图标签（UI-13）：同 planId 幂等只激活（切视图不触发新运行的
 * 状态层前提）；同会话换计划时先移除该会话旧图标签——每会话至多一个图视图，
 * 避免旧计划标签堆积。不同会话的图标签互不影响。
 */
export function openGraphTab(
  state: EditorTabsState,
  planId: string,
  sessionId: string,
): EditorTabsState {
  const id = graphTabId(planId);
  const existing = state.tabs.find((tab) => tab.id === id);
  if (existing) {
    return {
      tabs: state.tabs.map((tab) => (tab.id === id ? { ...tab, sessionId } : tab)),
      activeTabId: id,
    };
  }
  const tabs = [
    ...state.tabs.filter((tab) => !(tab.kind === "graph" && tab.sessionId === sessionId)),
    { id, kind: "graph" as const, planId, sessionId },
  ];
  return { tabs, activeTabId: id };
}

/**
 * 关闭标签：活动标签关闭后回退到相邻标签（优先右侧邻居，越界取末位），
 * 与既有 useProjectPanels 回退策略一致。
 */
export function closeTab(state: EditorTabsState, tabId: string): EditorTabsState {
  const index = state.tabs.findIndex((tab) => tab.id === tabId);
  if (index === -1) return state;
  const tabs = state.tabs.filter((tab) => tab.id !== tabId);
  if (state.activeTabId !== tabId) return { tabs, activeTabId: state.activeTabId };
  if (tabs.length === 0) return { tabs, activeTabId: null };
  const next = tabs[Math.min(index, tabs.length - 1)];
  return { tabs, activeTabId: next.id };
}

export function closeOtherTabs(state: EditorTabsState, tabId: string): EditorTabsState {
  if (!state.tabs.some((tab) => tab.id === tabId)) return state;
  const tabs = state.tabs.filter((tab) => tab.id === tabId);
  return { tabs, activeTabId: tabId };
}

export function closeTabsToRight(state: EditorTabsState, tabId: string): EditorTabsState {
  const index = state.tabs.findIndex((tab) => tab.id === tabId);
  if (index === -1) return state;
  const tabs = state.tabs.slice(0, index + 1);
  const activeTabId = tabs.some((tab) => tab.id === state.activeTabId)
    ? state.activeTabId
    : tabId;
  return { tabs, activeTabId };
}

export function closeAllTabs(): EditorTabsState {
  return { tabs: [], activeTabId: null };
}

/** 文件树重命名/删除同步：更新受影响文件标签的路径/名称或移除之。 */
export function renameFileTab(
  state: EditorTabsState,
  oldPath: string,
  newPath: string,
  newName: string,
): EditorTabsState {
  const id = fileTabId(oldPath);
  const tabs = state.tabs
    .map((tab) =>
      tab.id === id ? { ...tab, id: fileTabId(newPath), path: newPath, name: newName } : tab,
    )
    .map((tab) =>
      tab.kind === "diff" && tab.diff.kind === "file" && tab.diff.filePath === oldPath
        ? { ...tab, id: diffTabId({ ...tab.diff, filePath: newPath }), diff: { ...tab.diff, filePath: newPath } }
        : tab,
    );
  const active = state.activeTabId === id ? fileTabId(newPath) : state.activeTabId;
  return { tabs, activeTabId: tabs.some((t) => t.id === active) ? active : (tabs[0]?.id ?? null) };
}

export function deleteFileTab(state: EditorTabsState, path: string): EditorTabsState {
  const id = fileTabId(path);
  let next = closeTab(state, id);
  next = {
    ...next,
    tabs: next.tabs.filter(
      (tab) => !(tab.kind === "diff" && tab.diff.kind === "file" && tab.diff.filePath === path),
    ),
  };
  if (next.activeTabId && !next.tabs.some((tab) => tab.id === next.activeTabId)) {
    next = { ...next, activeTabId: next.tabs[0]?.id ?? null };
  }
  return next;
}

export function activeTab(state: EditorTabsState): EditorTab | null {
  return state.tabs.find((tab) => tab.id === state.activeTabId) ?? null;
}

export function fileTabs(state: EditorTabsState): Extract<EditorTab, { kind: "file" }>[] {
  return state.tabs.filter((tab): tab is Extract<EditorTab, { kind: "file" }> => tab.kind === "file");
}

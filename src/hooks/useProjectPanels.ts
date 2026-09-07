import { useState, useCallback, useRef } from "react";
import {
  BROWSER_TAB_ID,
  EMPTY_EDITOR_TABS,
  activeTab,
  closeAllTabs,
  closeOtherTabs,
  closeTab,
  closeTabsToRight,
  deleteFileTab,
  fileTabs,
  openBrowserTab,
  openDiffTab,
  openFileTab,
  openGraphTab,
  renameFileTab,
  selectTab,
  type EditorTab,
  type EditorTabsState,
} from "../components/project/main-tabs";
import { selectWorkspacePrefs, useWorkspaceStore } from "../stores/workspace-store";
import { TERMINAL_HEIGHT_LIMITS } from "../components/project/workspace-budget";

/**
 * 项目面板状态（UI-08 换底座，UI-18 收敛右面板）：终端高度等尺寸偏好由
 * workspace-store 按工作区持久化。旧右面板（files/git/browser）机制已删除——
 * 文件与 Git 走上下文导航，浏览器迁入主区标签（工作区单例）。
 * prefs.rightPanelWidth 成为孤儿偏好，清理登记于 UI-27。
 */
export function useProjectPanels(workspaceId: string) {
  const [editorWorkbenchVisible, setEditorWorkbenchVisible] = useState(true);
  // 文件、diff、执行图与浏览器统一标签体系（UI-09/13/18）。
  const [editorTabs, setEditorTabs] = useState<EditorTabsState>(EMPTY_EDITOR_TABS);
  const openFiles = fileTabs(editorTabs);
  const activeEditorTab = activeTab(editorTabs);
  const openDiff = activeEditorTab?.kind === "diff" ? activeEditorTab.diff : null;
  const activeFileTabId = activeEditorTab?.kind === "file" ? activeEditorTab.id : null;
  /**
   * 编辑区是否有内容（UI-13 收敛为单一派生值）：任何标签（文件/diff/执行图/
   * 浏览器）都算内容——ProjectPage 与 ProjectWorkbenchContent 不再各自复制表达式。
   */
  const hasEditorContent = editorTabs.tabs.length > 0;
  const prefs = useWorkspaceStore(selectWorkspacePrefs(workspaceId));
  /** 拖拽中的实时值；mouseup 才写回 store，避免高频持久化。 */
  const [dragTerminalHeight, setDragTerminalHeight] = useState<number | null>(null);
  const dragTerminalHeightRef = useRef(dragTerminalHeight);
  dragTerminalHeightRef.current = dragTerminalHeight;
  const terminalHeight = dragTerminalHeight ?? prefs.terminalHeight;
  const terminalHeightRef = useRef(terminalHeight);
  terminalHeightRef.current = terminalHeight;
  const setTerminalHeight = useCallback(
    (height: number) =>
      useWorkspaceStore.getState().setPrefs(workspaceId, { terminalHeight: height }),
    [workspaceId],
  );

  const handleFileSelect = useCallback((path: string, name: string) => {
    setEditorWorkbenchVisible(true);
    setEditorTabs((prev) => openFileTab(prev, path, name));
  }, []);

  const handleFileTabSelect = useCallback((tabId: string) => {
    setEditorTabs((prev) => selectTab(prev, tabId));
  }, []);

  const handleFileTabClose = useCallback((tabId: string) => {
    setEditorTabs((prev) => closeTab(prev, tabId));
  }, []);

  const handleCloseOtherFileTabs = useCallback((tabId: string) => {
    setEditorTabs((prev) => closeOtherTabs(prev, tabId));
  }, []);

  const handleCloseTabsToRight = useCallback((tabId: string) => {
    setEditorTabs((prev) => closeTabsToRight(prev, tabId));
  }, []);

  const handleCloseAllFileTabs = useCallback(() => {
    setEditorTabs(closeAllTabs());
  }, []);

  const handleFileTreeRename = useCallback((currentPath: string, nextPath: string) => {
    setEditorTabs((prev) => {
      const renamed = renameFileTab(prev, currentPath, nextPath, nextPath.split("/").pop() ?? nextPath);
      return renamed;
    });
  }, []);

  const handleFileTreeDelete = useCallback((deletedPath: string) => {
    setEditorTabs((prev) => deleteFileTab(prev, deletedPath));
  }, []);

  const handleDiffFileSelect = useCallback((filePath: string, staged: boolean, label: string) => {
    setEditorWorkbenchVisible(true);
    setEditorTabs((prev) => openDiffTab(prev, { kind: "file", filePath, staged, label }));
  }, []);

  const handleCommitSelect = useCallback((hash: string, message: string) => {
    setEditorWorkbenchVisible(true);
    setEditorTabs((prev) => openDiffTab(prev, { kind: "commit", hash, message }));
  }, []);

  const handleCommitFileClick = useCallback((hash: string, filePath: string, label: string) => {
    setEditorWorkbenchVisible(true);
    setEditorTabs((prev) => openDiffTab(prev, { kind: "commit-file", hash, filePath, label }));
  }, []);

  const handleCloseActiveDiff = useCallback(() => {
    setEditorTabs((prev) => (prev.activeTabId ? closeTab(prev, prev.activeTabId) : prev));
  }, []);

  /** 打开/激活执行图标签（UI-13）；同 planId 幂等，reducer 保证切视图不新建。 */
  const handleOpenGraphTab = useCallback((planId: string, sessionId: string) => {
    setEditorWorkbenchVisible(true);
    setEditorTabs((prev) => openGraphTab(prev, planId, sessionId));
  }, []);

  const handleCloseGraphTab = useCallback((tabId: string) => {
    setEditorTabs((prev) => closeTab(prev, tabId));
  }, []);

  /** 打开/激活浏览器预览标签（UI-18，工作区单例）。 */
  const handleOpenBrowserTab = useCallback(() => {
    setEditorWorkbenchVisible(true);
    setEditorTabs((prev) => openBrowserTab(prev));
  }, []);

  /** 关闭浏览器标签 = 隐藏面板；不停进程（browser_stop 只在面板头部/dock 触发）。 */
  const handleCloseBrowserTab = useCallback(() => {
    setEditorTabs((prev) => closeTab(prev, BROWSER_TAB_ID));
  }, []);

  const hideEditorWorkbench = useCallback(() => {
    setEditorWorkbenchVisible(false);
  }, []);

  const showEditorWorkbench = useCallback(() => {
    setEditorWorkbenchVisible(true);
  }, []);

  const clearFileAndDiff = useCallback(() => {
    setEditorTabs(closeAllTabs());
  }, []);

  const handleTerminalResizeStart = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    const startY = e.clientY;
    const startHeight = terminalHeightRef.current;
    const onMouseMove = (ev: MouseEvent) => {
      // 与 workspace-prefs sanitize / budget chrome 同一出处的钳制（UI-19）。
      const newHeight = Math.max(
        TERMINAL_HEIGHT_LIMITS.min,
        Math.min(
          TERMINAL_HEIGHT_LIMITS.max,
          startHeight + (startY - ev.clientY),
        ),
      );
      setDragTerminalHeight(newHeight);
    };
    const onMouseUp = () => {
      document.removeEventListener("mousemove", onMouseMove);
      document.removeEventListener("mouseup", onMouseUp);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      const latestHeight = dragTerminalHeightRef.current;
      setDragTerminalHeight(null);
      if (latestHeight != null) setTerminalHeight(latestHeight);
    };
    document.body.style.cursor = "row-resize";
    document.body.style.userSelect = "none";
    document.addEventListener("mousemove", onMouseMove);
    document.addEventListener("mouseup", onMouseUp);
  }, [setTerminalHeight]);

  return {
    editorWorkbenchVisible,
    openFiles,
    activeFileTabId,
    activeEditorTab,
    openDiff,
    editorTabs,
    hasEditorContent,
    terminalHeight,
    handleCloseActiveDiff,
    handleFileSelect,
    handleFileTabSelect,
    handleFileTabClose,
    handleCloseOtherFileTabs,
    handleCloseTabsToRight,
    handleCloseAllFileTabs,
    handleFileTreeRename,
    handleFileTreeDelete,
    handleDiffFileSelect,
    handleCommitSelect,
    handleCommitFileClick,
    handleOpenGraphTab,
    handleCloseGraphTab,
    handleOpenBrowserTab,
    handleCloseBrowserTab,
    hideEditorWorkbench,
    showEditorWorkbench,
    clearFileAndDiff,
    handleTerminalResizeStart,
    /** 终端高度即时提交（UI-23c 键盘步进 / 双击复位走此通道；拖拽仍 mouseup 提交）。 */
    commitTerminalHeight: setTerminalHeight,
  };
}
export type { EditorTab, EditorTabsState };

import { useState, useCallback, useRef } from "react";
import { type RightPanel } from "./projectPanelsFileState";
import {
  EMPTY_EDITOR_TABS,
  activeTab,
  closeAllTabs,
  closeOtherTabs,
  closeTab,
  closeTabsToRight,
  deleteFileTab,
  fileTabs,
  openDiffTab,
  openFileTab,
  renameFileTab,
  selectTab,
  type EditorTab,
  type EditorTabsState,
} from "../components/project/main-tabs";
import { useDockedBrowserPanel } from "./useDockedBrowserPanel";
import { selectWorkspacePrefs, useWorkspaceStore } from "../stores/workspace-store";

/**
 * 项目面板状态（UI-08 换底座）：右栏宽/终端高度等尺寸偏好改由
 * workspace-store 按工作区持久化；签名保持兼容，消费组件零改动。
 */
export function useProjectPanels(workspaceId: string) {
  const [rightPanel, setRightPanel] = useState<RightPanel>(null);
  const [editorWorkbenchVisible, setEditorWorkbenchVisible] = useState(true);
  // 文件与 diff 统一标签体系（UI-09）：diff 不再是互斥独立槽。
  const [editorTabs, setEditorTabs] = useState<EditorTabsState>(EMPTY_EDITOR_TABS);
  const openFiles = fileTabs(editorTabs);
  const activeEditorTab = activeTab(editorTabs);
  const openDiff = activeEditorTab?.kind === "diff" ? activeEditorTab.diff : null;
  const activeFileTabId = activeEditorTab?.kind === "file" ? activeEditorTab.id : null;
  const prefs = useWorkspaceStore(selectWorkspacePrefs(workspaceId));
  /** 拖拽中的实时值；mouseup 才写回 store，避免高频持久化。 */
  const [dragRightWidth, setDragRightWidth] = useState<number | null>(null);
  const [dragTerminalHeight, setDragTerminalHeight] = useState<number | null>(null);
  const dragRightWidthRef = useRef(dragRightWidth);
  dragRightWidthRef.current = dragRightWidth;
  const dragTerminalHeightRef = useRef(dragTerminalHeight);
  dragTerminalHeightRef.current = dragTerminalHeight;
  const rightPanelWidth = dragRightWidth ?? prefs.rightPanelWidth;
  const terminalHeight = dragTerminalHeight ?? prefs.terminalHeight;
  const setRightPanelWidth = useCallback(
    (width: number) =>
      useWorkspaceStore.getState().setPrefs(workspaceId, { rightPanelWidth: width }),
    [workspaceId],
  );
  const setTerminalHeight = useCallback(
    (height: number) =>
      useWorkspaceStore.getState().setPrefs(workspaceId, { terminalHeight: height }),
    [workspaceId],
  );
  const browserPanel = useDockedBrowserPanel("nezha.project.browserPanelWidth");
  const rightPanelRef = useRef(rightPanel);
  const rightPanelWidthRef = useRef(rightPanelWidth);
  rightPanelRef.current = rightPanel;
  rightPanelWidthRef.current = rightPanelWidth;
  const terminalHeightRef = useRef(terminalHeight);
  terminalHeightRef.current = terminalHeight;

  const handleTogglePanel = useCallback((panel: Exclude<RightPanel, null>) => {
    setRightPanel((prev) => (prev === panel ? null : panel));
  }, []);

  const handleOpenPanel = useCallback((panel: Exclude<RightPanel, null>) => {
    setRightPanel(panel);
  }, []);

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

  const hideEditorWorkbench = useCallback(() => {
    setEditorWorkbenchVisible(false);
  }, []);

  const showEditorWorkbench = useCallback(() => {
    setEditorWorkbenchVisible(true);
  }, []);

  const clearFileAndDiff = useCallback(() => {
    setEditorTabs(closeAllTabs());
  }, []);

  const handleRightResizeStart = useCallback((e: React.MouseEvent) => {
    if (rightPanelRef.current === "browser") {
      browserPanel.handleResizeStart(e);
      return;
    }

    e.preventDefault();
    const startX = e.clientX;
    const startWidth = rightPanelWidthRef.current;
    const onMouseMove = (ev: MouseEvent) => {
      const newWidth = Math.max(180, Math.min(600, startWidth + (startX - ev.clientX)));
      setDragRightWidth(newWidth);
    };
    const onMouseUp = () => {
      document.removeEventListener("mousemove", onMouseMove);
      document.removeEventListener("mouseup", onMouseUp);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      const latest = dragRightWidthRef.current;
      setDragRightWidth(null);
      if (latest != null) setRightPanelWidth(latest);
    };
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
    document.addEventListener("mousemove", onMouseMove);
    document.addEventListener("mouseup", onMouseUp);
  }, [browserPanel, setRightPanelWidth]);

  /** 键盘/复位等离散调整入口（拖拽走 handleRightResizeStart）。 */
  const applyRightPanelWidth = useCallback((width: number) => {
    setRightPanelWidth(Math.max(180, Math.min(600, Math.round(width))));
  }, [setRightPanelWidth]);

  const handleTerminalResizeStart = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    const startY = e.clientY;
    const startHeight = terminalHeightRef.current;
    const onMouseMove = (ev: MouseEvent) => {
      const newHeight = Math.max(100, Math.min(600, startHeight + (startY - ev.clientY)));
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
    rightPanel,
    editorWorkbenchVisible,
    openFiles,
    activeFileTabId,
    activeEditorTab,
    openDiff,
    rightPanelWidth: rightPanel === "browser" ? browserPanel.effectiveWidth : rightPanelWidth,
    browserPanelExpanded: browserPanel.expanded,
    terminalHeight,
    handleCloseActiveDiff,
    handleTogglePanel,
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
    hideEditorWorkbench,
    showEditorWorkbench,
    clearFileAndDiff,
    handleRightResizeStart,
    applyRightPanelWidth,
    handleToggleBrowserPanelExpanded: browserPanel.toggleExpanded,
    handleTerminalResizeStart,
    handleOpenPanel,
  };
}
export type { EditorTab, EditorTabsState };

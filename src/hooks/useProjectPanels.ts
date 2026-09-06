import { useState, useCallback, useRef } from "react";
import {
  deleteFromOpenFilesState,
  deleteOpenDiff,
  renameOpenDiff,
  renameOpenFilesState,
  type OpenDiff,
  type OpenFileTab,
  type RightPanel,
} from "./projectPanelsFileState";
import { useDockedBrowserPanel } from "./useDockedBrowserPanel";
import { selectWorkspacePrefs, useWorkspaceStore } from "../stores/workspace-store";

/**
 * 项目面板状态（UI-08 换底座）：右栏宽/终端高度等尺寸偏好改由
 * workspace-store 按工作区持久化；签名保持兼容，消费组件零改动。
 */
export function useProjectPanels(workspaceId: string) {
  const [rightPanel, setRightPanel] = useState<RightPanel>(null);
  const [editorWorkbenchVisible, setEditorWorkbenchVisible] = useState(true);
  const [openFilesState, setOpenFilesState] = useState<{
    tabs: OpenFileTab[];
    activeTabId: string | null;
  }>({
    tabs: [],
    activeTabId: null,
  });
  const [openDiff, setOpenDiff] = useState<OpenDiff | null>(null);
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
  const nextFileTabIdRef = useRef(0);

  const handleTogglePanel = useCallback((panel: Exclude<RightPanel, null>) => {
    setRightPanel((prev) => (prev === panel ? null : panel));
  }, []);

  const handleOpenPanel = useCallback((panel: Exclude<RightPanel, null>) => {
    setRightPanel(panel);
  }, []);

  const handleFileSelect = useCallback((path: string, name: string) => {
    setEditorWorkbenchVisible(true);
    setOpenDiff(null);
    setOpenFilesState((prev) => {
      const existingTab = prev.tabs.find((tab) => tab.path === path);
      if (existingTab) {
        return {
          tabs: prev.tabs,
          activeTabId: existingTab.id,
        };
      }

      const nextTab: OpenFileTab = {
        id: `file-tab-${nextFileTabIdRef.current++}`,
        path,
        name,
      };

      return {
        tabs: [...prev.tabs, nextTab],
        activeTabId: nextTab.id,
      };
    });
  }, []);

  const handleFileTabSelect = useCallback((tabId: string) => {
    setOpenFilesState((prev) => ({
      tabs: prev.tabs,
      activeTabId: prev.tabs.some((tab) => tab.id === tabId) ? tabId : prev.activeTabId,
    }));
  }, []);

  const handleFileTabClose = useCallback((tabId: string) => {
    setOpenFilesState((prev) => {
      const closingIndex = prev.tabs.findIndex((tab) => tab.id === tabId);
      if (closingIndex === -1) return prev;

      const nextTabs = prev.tabs.filter((tab) => tab.id !== tabId);
      const nextActiveTabId =
        prev.activeTabId !== tabId
          ? prev.activeTabId
          : nextTabs[Math.min(closingIndex, nextTabs.length - 1)]?.id ?? null;

      return {
        tabs: nextTabs,
        activeTabId: nextActiveTabId,
      };
    });
  }, []);

  const handleCloseOtherFileTabs = useCallback((tabId: string) => {
    setOpenFilesState((prev) => {
      const activeTab = prev.tabs.find((tab) => tab.id === tabId);
      if (!activeTab) return prev;
      return {
        tabs: [activeTab],
        activeTabId: activeTab.id,
      };
    });
  }, []);

  const handleCloseTabsToRight = useCallback((tabId: string) => {
    setOpenFilesState((prev) => {
      const activeIndex = prev.tabs.findIndex((tab) => tab.id === tabId);
      if (activeIndex === -1) return prev;

      const nextTabs = prev.tabs.slice(0, activeIndex + 1);
      return {
        tabs: nextTabs,
        activeTabId: nextTabs.some((tab) => tab.id === prev.activeTabId) ? prev.activeTabId : tabId,
      };
    });
  }, []);

  const handleCloseAllFileTabs = useCallback(() => {
    setOpenFilesState({
      tabs: [],
      activeTabId: null,
    });
  }, []);

  const handleFileTreeRename = useCallback((currentPath: string, nextPath: string) => {
    setOpenFilesState((prev) => renameOpenFilesState(prev, currentPath, nextPath));
    setOpenDiff((prev) => renameOpenDiff(prev, currentPath, nextPath));
  }, []);

  const handleFileTreeDelete = useCallback((deletedPath: string) => {
    setOpenFilesState((prev) => deleteFromOpenFilesState(prev, deletedPath));
    setOpenDiff((prev) => deleteOpenDiff(prev, deletedPath));
  }, []);

  const handleDiffFileSelect = useCallback((filePath: string, staged: boolean, label: string) => {
    setEditorWorkbenchVisible(true);
    setOpenDiff({ kind: "file", filePath, staged, label });
  }, []);

  const handleCommitSelect = useCallback((hash: string, message: string) => {
    setEditorWorkbenchVisible(true);
    setOpenDiff({ kind: "commit", hash, message });
  }, []);

  const handleCommitFileClick = useCallback((hash: string, filePath: string, label: string) => {
    setEditorWorkbenchVisible(true);
    setOpenDiff({ kind: "commit-file", hash, filePath, label });
  }, []);

  const hideEditorWorkbench = useCallback(() => {
    setEditorWorkbenchVisible(false);
  }, []);

  const showEditorWorkbench = useCallback(() => {
    setEditorWorkbenchVisible(true);
  }, []);

  const clearFileAndDiff = useCallback(() => {
    setOpenFilesState({
      tabs: [],
      activeTabId: null,
    });
    setOpenDiff(null);
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
    openFiles: openFilesState.tabs,
    activeFileTabId: openFilesState.activeTabId,
    openDiff,
    rightPanelWidth: rightPanel === "browser" ? browserPanel.effectiveWidth : rightPanelWidth,
    browserPanelExpanded: browserPanel.expanded,
    terminalHeight,
    setOpenDiff,
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
export type { OpenFileTab };

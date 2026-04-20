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

export function useProjectPanels() {
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
  const [rightPanelWidth, setRightPanelWidth] = useState(280);
  const [terminalHeight, setTerminalHeight] = useState(240);
  const rightPanelWidthRef = useRef(rightPanelWidth);
  rightPanelWidthRef.current = rightPanelWidth;
  const terminalHeightRef = useRef(terminalHeight);
  terminalHeightRef.current = terminalHeight;
  const nextFileTabIdRef = useRef(0);

  const handleTogglePanel = useCallback((panel: "files" | "git-changes" | "git-history") => {
    setRightPanel((prev) => (prev === panel ? null : panel));
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
    e.preventDefault();
    const startX = e.clientX;
    const startWidth = rightPanelWidthRef.current;
    const onMouseMove = (ev: MouseEvent) => {
      const newWidth = Math.max(180, Math.min(600, startWidth + (startX - ev.clientX)));
      setRightPanelWidth(newWidth);
    };
    const onMouseUp = () => {
      document.removeEventListener("mousemove", onMouseMove);
      document.removeEventListener("mouseup", onMouseUp);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
    document.addEventListener("mousemove", onMouseMove);
    document.addEventListener("mouseup", onMouseUp);
  }, []);

  const handleTerminalResizeStart = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    const startY = e.clientY;
    const startHeight = terminalHeightRef.current;
    const onMouseMove = (ev: MouseEvent) => {
      const newHeight = Math.max(100, Math.min(600, startHeight + (startY - ev.clientY)));
      setTerminalHeight(newHeight);
    };
    const onMouseUp = () => {
      document.removeEventListener("mousemove", onMouseMove);
      document.removeEventListener("mouseup", onMouseUp);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
    document.body.style.cursor = "row-resize";
    document.body.style.userSelect = "none";
    document.addEventListener("mousemove", onMouseMove);
    document.addEventListener("mouseup", onMouseUp);
  }, []);

  return {
    rightPanel,
    editorWorkbenchVisible,
    openFiles: openFilesState.tabs,
    activeFileTabId: openFilesState.activeTabId,
    openDiff,
    rightPanelWidth,
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
    handleTerminalResizeStart,
  };
}
export type { OpenFileTab };

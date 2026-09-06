import { lazy, Suspense, useCallback, useEffect, useState } from "react";
import type { Project } from "../types";
import { SessionPanel } from "./SessionPanel";
import { PanelLeftOpen } from "lucide-react";
import { AppRail } from "./shell/AppRail";
import { ContextNav, type ContextNavTab } from "./shell/ContextNav";
import { StatusDockBar } from "./shell/StatusDockBar";
import { ErrorBoundary } from "./ErrorBoundary";
import { useProjectPanels } from "../hooks/useProjectPanels";
import { useBrowserSessionDock } from "../hooks/useBrowserSessionDock";
import { useGraphTabSync } from "../hooks/useGraphTabSync";
import { useWorkspaceBudget } from "../hooks/useWorkspaceBudget";
import { selectWorkspacePrefs, useWorkspaceStore } from "../stores/workspace-store";
import type { WorkspacePrefs } from "./project/workspace-prefs";
import { useProjectMcpStatus } from "../hooks/use-mcp-status";
import {
  ProjectMainArea,
  ProjectWorkspaceLayout,
} from "./project/ProjectWorkspaceLayout";
import { ProjectLazyPaneFallback } from "./project/ProjectLazyPaneFallback";
import { ProjectOverlays } from "./project/ProjectOverlays";
import { ProjectWorkbenchContent } from "./project/ProjectWorkbenchContent";

const FileExplorer = lazy(() =>
  import("./FileExplorer").then((module) => ({ default: module.FileExplorer })),
);
const GitChanges = lazy(() =>
  import("./GitChanges").then((module) => ({ default: module.GitChanges })),
);
const GitHistory = lazy(() =>
  import("./GitHistory").then((module) => ({ default: module.GitHistory })),
);
const ShellTerminalPanel = lazy(() =>
  import("./ShellTerminalPanel").then((module) => ({ default: module.ShellTerminalPanel })),
);

export function ProjectPage({
  project,
  visible = true,
  allProjects = [],
  openProjects,
  onBack,
  onNavigateHome,
  onSwitchProject,
  onCloseProject,
  onOpen,
}: {
  project: Project;
  visible?: boolean;
  allProjects?: Project[];
  /** 当前已打开（挂载）的项目窗口，展示在左侧项目栏。必传：项目栏语义
   * 是「仅展示已打开的窗口」，不在组件内回退 allProjects，避免两套语义。 */
  openProjects: Project[];
  onBack: () => void;
  /** 全局 rail 入口：定向返回欢迎页的聊天/项目/架构空间（UI-07）。 */
  onNavigateHome?: (space: "chat" | "projects" | "architecture") => void;
  onSwitchProject: (project: Project) => void;
  onCloseProject?: (project: Project) => void;
  onOpen: () => void;
}) {
  const panels = useProjectPanels(project.id);
  const {
    openFiles,
    terminalHeight,
    handleFileSelect,
    handleFileTreeRename,
    handleFileTreeDelete,
    handleDiffFileSelect,
    handleCommitSelect,
    handleCommitFileClick,
    handleTerminalResizeStart,
  } = panels;

  const [showShellTerminal, setShowShellTerminal] = useState(false);
  const [showDispatcherSettings, setShowDispatcherSettings] = useState(false);
  const [showMcpStatus, setShowMcpStatus] = useState(false);
  const {
    status: mcpStatus,
    checking: mcpChecking,
    updatingServer: mcpUpdatingServer,
    refresh: refreshMcpStatus,
    setServerEnabled: toggleMcpServerEnabled,
  } = useProjectMcpStatus(project.path, visible);
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [sessionWorkbenchVisible, setSessionWorkbenchVisible] = useState(true);
  // 布局偏好按工作区持久化（UI-08）；setter 经 store 校验后落盘。
  const workspacePrefs = useWorkspaceStore(selectWorkspacePrefs(project.id));
  const sessionSidebarCollapsed = workspacePrefs.sessionSidebarCollapsed;
  const editorPaneRatio = workspacePrefs.editorPaneRatio;
  const contextTab = workspacePrefs.contextTab;
  const contextNavWidth = workspacePrefs.contextNavWidth;
  const setWorkspacePref = useCallback(
    (patch: Partial<WorkspacePrefs>) => useWorkspaceStore.getState().setPrefs(project.id, patch),
    [project.id],
  );
  const setSessionSidebarCollapsed = useCallback(
    (value: boolean) => setWorkspacePref({ sessionSidebarCollapsed: value }),
    [setWorkspacePref],
  );
  const setEditorPaneRatio = useCallback(
    (value: number) => setWorkspacePref({ editorPaneRatio: value }),
    [setWorkspacePref],
  );
  const setContextTab = useCallback(
    (value: ContextNavTab) => setWorkspacePref({ contextTab: value }),
    [setWorkspacePref],
  );
  const setContextNavWidth = useCallback(
    (value: number) => setWorkspacePref({ contextNavWidth: value }),
    [setWorkspacePref],
  );

    // 空间预算（UI-04）：偏好为冻结输入，窄窗临时适配只体现在 budget 输出。
  // 编辑区内容判定收敛到 panels 单一派生值（UI-13）。
  const hasEditorContent = panels.hasEditorContent;
  // UI-18：右面板已迁入主区标签，预算的 rightPanel 通道恒关（纯函数与
  // 参数化测试不动——rightPanel 降级能力成为无消费者的通用能力，
  // 移除评估登记 UI-27）。
  const budget = useWorkspaceBudget({
    navOpen: !sessionSidebarCollapsed,
    navWidthPref: contextNavWidth,
    rightPanelOpen: false,
    rightPanelWidthPref: 0,
    terminalOpen: showShellTerminal,
    terminalHeightPref: terminalHeight,
    dualPaneRequested: sessionWorkbenchVisible && hasEditorContent,
    editorRatioPref: editorPaneRatio,
  });

  const handleSelectSession = useCallback((sessionId: string | null) => {
    if (sessionId) setSessionWorkbenchVisible(true);
    setActiveSessionId(sessionId);
  }, []);

  // 执行图意图 → 主区标签同步（UI-13）：store 的 graphPanel 是打开意图，
  // 标签是渲染真值；关标签时按会话+计划匹配清除意图。
  const { closeGraphTab } = useGraphTabSync({
    activeSessionId,
    editorTabs: panels.editorTabs,
    onOpenGraphTab: panels.handleOpenGraphTab,
    onCloseGraphTab: panels.handleCloseGraphTab,
  });
  // 扩大/还原主区：收起/恢复会话 pane（执行图与浏览器标签共用，切换不触发重跑）。
  const handleExpandMainArea = useCallback(() => {
    setSessionWorkbenchVisible((visible) => !visible);
  }, []);

  // Files 面板是 lazy 块（含 seti 图标 eager 资源），首次打开才求值会卡顿；
  // 挂载后的空闲窗口预取，让首次点击命中缓存。
  useEffect(() => {
    if (!visible) return;
    const timer = window.setTimeout(() => {
      void import("./FileExplorer");
    }, 600);
    return () => window.clearTimeout(timer);
  }, [visible]);

  // 浏览器 = 主区标签（UI-18）：开/关标签只影响视图；进程生命周期由
  // 面板头部「关闭浏览器」与 dock 的 browser_stop 承担（隐藏 ≠ 结束）。
  const { activeEditorTab, handleOpenBrowserTab, handleCloseBrowserTab } = panels;
  // 导航列表 ↔ 主区 diff 对应（UI-17）：变更页按 (path, staged)、历史页按 hash 高亮。
  const activeDiffTab =
    activeEditorTab?.kind === "diff" ? activeEditorTab.diff : null;
  const activeFileDiff =
    activeDiffTab?.kind === "file"
      ? { path: activeDiffTab.filePath, staged: activeDiffTab.staged }
      : null;
  const activeCommitHash =
    activeDiffTab?.kind === "commit" || activeDiffTab?.kind === "commit-file"
      ? activeDiffTab.hash
      : null;
  const openBrowserPanel = useCallback(
    () => handleOpenBrowserTab(),
    [handleOpenBrowserTab],
  );
  const minimizeBrowserPanel = useCallback(() => {
    if (activeEditorTab?.kind === "browser") handleCloseBrowserTab();
  }, [activeEditorTab, handleCloseBrowserTab]);
  const {
    dockedSessions,
    minimize: handleMinimizeBrowser,
    restore: handleRestoreBrowser,
    closeDocked: handleCloseDockedBrowser,
    reopen: handleReopenBrowser,
    openUrl: handleOpenMarkdownLink,
  } = useBrowserSessionDock({
    activeSessionId,
    projectPath: project.path,
    onOpen: openBrowserPanel,
    onMinimized: minimizeBrowserPanel,
    onRestoreSession: handleSelectSession,
    enabled: visible,
  });

  const railNode = (
    <AppRail
      space="project"
      onNavigateHome={onNavigateHome}
      projectId={project.id}
      projectPath={project.path}
    />
  );

  const sessionPanelNode =
    !sessionSidebarCollapsed && budget.navWidth > 0 ? (
    <ContextNav
      project={project}
      openProjects={openProjects}
      allProjects={allProjects}
      activeTab={contextTab}
      onTabChange={setContextTab}
      width={contextNavWidth}
      onWidthCommit={setContextNavWidth}
      onSwitchProject={onSwitchProject}
      onCloseProject={(target) => onCloseProject?.(target)}
      onOpenProject={onOpen}
      onCollapse={() => setSessionSidebarCollapsed(true)}
      sessionContent={
        <SessionPanel
          project={project}
          activeSessionId={activeSessionId}
          onSelectSession={handleSelectSession}
          onBack={onBack}
          onCollapse={() => setSessionSidebarCollapsed(true)}
          hideChrome
        />
      }
      filesContent={
        <ErrorBoundary label="文件浏览器">
          <Suspense fallback={<ProjectLazyPaneFallback label="文件列表加载中..." />}>
            <FileExplorer
              projectPath={project.path}
              onFileSelect={handleFileSelect}
              onFileRename={handleFileTreeRename}
              onFileDelete={handleFileTreeDelete}
              openFilePaths={openFiles.map((tab) => tab.path)}
              active={visible}
            />
          </Suspense>
        </ErrorBoundary>
      }
      changesContent={
        <ErrorBoundary label="Git 变更">
          <Suspense fallback={<ProjectLazyPaneFallback label="Git 变更加载中..." />}>
            <GitChanges
              projectPath={project.path}
              onFileSelect={handleDiffFileSelect}
              activeFileDiff={activeFileDiff}
            />
          </Suspense>
        </ErrorBoundary>
      }
      historyContent={
        <ErrorBoundary label="Git 历史">
          <Suspense fallback={<ProjectLazyPaneFallback label="Git 历史加载中..." />}>
            <GitHistory
              projectPath={project.path}
              onCommitSelect={handleCommitSelect}
              onFileClick={handleCommitFileClick}
              activeCommitHash={activeCommitHash}
            />
          </Suspense>
        </ErrorBoundary>
      }
    />
    ) : (
      <div className="ai-context-nav-collapsed">
        <button
          type="button"
          className="ai-context-nav-collapse"
          aria-label="展开导航"
          title="展开导航"
          onClick={() => setSessionSidebarCollapsed(false)}
        >
          <PanelLeftOpen size={14} strokeWidth={2} />
        </button>
      </div>
    );

  const workbenchNode = (
    <ProjectWorkbenchContent
      project={project}
      activeSessionId={activeSessionId}
      mcpStatus={mcpStatus}
      mcpChecking={mcpChecking}
      panels={panels}
      sessionWorkbenchVisible={sessionWorkbenchVisible}
      onSessionWorkbenchVisibleChange={setSessionWorkbenchVisible}
      onSelectSession={handleSelectSession}
      onOpenMarkdownLink={handleOpenMarkdownLink}
      onOpenMcpStatus={() => setShowMcpStatus(true)}
      onOpenSettings={() => setShowDispatcherSettings(true)}
      workspaceVisible={visible}
      budget={budget}
      editorPaneRatio={editorPaneRatio}
      onEditorPaneRatioChange={setEditorPaneRatio}
      onCloseGraphTab={closeGraphTab}
      onExpandMainArea={handleExpandMainArea}
      onMinimizeBrowser={handleMinimizeBrowser}
      onReopenBrowser={handleReopenBrowser}
    />
  );

  const shellTerminalNode = showShellTerminal ? (
    <Suspense fallback={null}>
      <ShellTerminalPanel
        projectPath={project.path}
        projectId={project.id}
        isActive={visible}
        onClose={() => setShowShellTerminal(false)}
        height={budget.terminalHeight}
        onResizeStart={handleTerminalResizeStart}
      />
    </Suspense>
  ) : undefined;

  const statusDockNode = (
    <StatusDockBar
      terminalActive={showShellTerminal}
      onToggleTerminal={() => setShowShellTerminal((value) => !value)}
      browserActive={panels.activeEditorTab?.kind === "browser"}
      onToggleBrowser={() =>
        panels.activeEditorTab?.kind === "browser"
          ? panels.handleCloseBrowserTab()
          : panels.handleOpenBrowserTab()
      }
      statusText={project.name}
    />
  );

  const mainNode = (
    <ProjectMainArea
      workbench={workbenchNode}
      shellTerminal={shellTerminalNode}
      statusDock={statusDockNode}
      mainStyle={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
        background: "var(--bg-panel)",
      }}
    />
  );

  const overlayNode = (
    <ProjectOverlays
      project={project}
      showSettings={showDispatcherSettings}
      showMcpStatus={showMcpStatus}
      mcpStatus={mcpStatus}
      mcpChecking={mcpChecking}
      mcpUpdatingServer={mcpUpdatingServer}
      dockedSessions={dockedSessions}
      onCloseSettings={() => setShowDispatcherSettings(false)}
      onCloseMcpStatus={() => setShowMcpStatus(false)}
      onRefreshMcpStatus={() => {
        refreshMcpStatus().catch(console.error);
      }}
      onToggleMcpServer={(serverName, enabled) => {
        toggleMcpServerEnabled(serverName, enabled).catch(console.error);
      }}
      onRestoreBrowser={handleRestoreBrowser}
      onCloseBrowser={handleCloseDockedBrowser}
    />
  );

  return (
    <ProjectWorkspaceLayout
      visible={visible}
      rootStyle={{ flex: 1, display: "flex", overflow: "hidden" }}
      rail={railNode}
      sessionPanel={sessionPanelNode}
      main={mainNode}
      overlays={overlayNode}
    />
  );
}

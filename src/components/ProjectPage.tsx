import { lazy, Suspense, useCallback, useEffect, useState } from "react";
import type { Project } from "../types";
import { SessionPanel } from "./SessionPanel";
import { PanelLeftOpen } from "lucide-react";
import { AppRail } from "./shell/AppRail";
import { ContextNav, type ContextNavTab } from "./shell/ContextNav";
import { RightToolbar } from "./RightToolbar";
import { ErrorBoundary } from "./ErrorBoundary";
import { useProjectPanels } from "../hooks/useProjectPanels";
import { useBrowserSessionDock } from "../hooks/useBrowserSessionDock";
import { useWorkspaceBudget } from "../hooks/useWorkspaceBudget";
import { useProjectMcpStatus } from "../hooks/use-mcp-status";
import {
  ProjectMainArea,
  ProjectRightPanelHost,
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
const BrowserPanel = lazy(() =>
  import("./BrowserPanel").then((module) => ({ default: module.BrowserPanel })),
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
  const panels = useProjectPanels();
  const {
    rightPanel,
    openFiles,
    rightPanelWidth,
    browserPanelExpanded,
    terminalHeight,
    handleTogglePanel,
    handleFileSelect,
    handleFileTreeRename,
    handleFileTreeDelete,
    handleDiffFileSelect,
    handleCommitSelect,
    handleCommitFileClick,
    handleRightResizeStart,
    handleToggleBrowserPanelExpanded,
    handleTerminalResizeStart,
    handleOpenPanel,
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
  const [sessionSidebarCollapsed, setSessionSidebarCollapsed] = useState(false);
  const [sessionWorkbenchVisible, setSessionWorkbenchVisible] = useState(true);
  const [editorPaneRatio, setEditorPaneRatio] = useState(0.5);
  const [contextTab, setContextTab] = useState<ContextNavTab>("sessions");
  const [contextNavWidth, setContextNavWidth] = useState(248);

  // 空间预算（UI-04）：偏好为冻结输入，窄窗临时适配只体现在 budget 输出。
  const hasEditorContent = panels.openDiff !== null || openFiles.length > 0;
  const budget = useWorkspaceBudget({
    navOpen: !sessionSidebarCollapsed,
    navWidthPref: contextNavWidth,
    rightPanelOpen: rightPanel !== null,
    rightPanelWidthPref: rightPanelWidth,
    terminalOpen: showShellTerminal,
    terminalHeightPref: terminalHeight,
    dualPaneRequested: sessionWorkbenchVisible && hasEditorContent,
    editorRatioPref: editorPaneRatio,
  });
  // 浏览器 expanded 是既有的视口比例语义（UI-18 迁移前保留），不走预算夹取。
  const rightPanelRenderWidth =
    rightPanel === "browser" && browserPanelExpanded
      ? rightPanelWidth
      : budget.rightPanelWidth;

  const handleSelectSession = useCallback((sessionId: string | null) => {
    if (sessionId) setSessionWorkbenchVisible(true);
    setActiveSessionId(sessionId);
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

  const openBrowserPanel = useCallback(() => handleOpenPanel("browser"), [handleOpenPanel]);
  const minimizeBrowserPanel = useCallback(() => {
    if (rightPanel === "browser") handleTogglePanel("browser");
  }, [handleTogglePanel, rightPanel]);
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
              projectName={project.name}
              onFileSelect={handleFileSelect}
              onFileRename={handleFileTreeRename}
              onFileDelete={handleFileTreeDelete}
              openFilePaths={openFiles.map((tab) => tab.path)}
              active={visible}
              width={contextNavWidth}
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
              width={contextNavWidth}
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
              width={contextNavWidth}
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

  const mainNode = (
    <ProjectMainArea
      workbench={workbenchNode}
      shellTerminal={shellTerminalNode}
      mainStyle={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
        background: "var(--bg-panel)",
      }}
    />
  );

  const rightPanelNode =
    rightPanel === "browser" && rightPanelRenderWidth > 0 ? (
      <ProjectRightPanelHost
        onResizeStart={handleRightResizeStart}
        onResizeKey={(event) => {
          const step = event.shiftKey ? 48 : 16;
          if (event.key === "ArrowLeft") {
            event.preventDefault();
            panels.applyRightPanelWidth(rightPanelWidth + step);
          } else if (event.key === "ArrowRight") {
            event.preventDefault();
            panels.applyRightPanelWidth(rightPanelWidth - step);
          }
        }}
        onResizeDoubleClick={() => panels.applyRightPanelWidth(280)}
      >
        <ErrorBoundary label="CloakBrowser">
          <Suspense fallback={<ProjectLazyPaneFallback label="浏览器加载中..." />}>
            <BrowserPanel
              sessionId={activeSessionId}
              projectPath={project.path}
              width={rightPanelRenderWidth}
              active={visible}
              expanded={browserPanelExpanded}
              onToggleExpanded={handleToggleBrowserPanelExpanded}
              onClose={() => handleTogglePanel("browser")}
              onMinimize={handleMinimizeBrowser}
              onReopen={handleReopenBrowser}
            />
          </Suspense>
        </ErrorBoundary>
      </ProjectRightPanelHost>
    ) : undefined;

  const toolbarNode = (
    <RightToolbar
      activePanel={rightPanel}
      onToggle={handleTogglePanel}
      terminalActive={showShellTerminal}
      onToggleTerminal={() => setShowShellTerminal((value) => !value)}
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
      rightPanel={rightPanelNode}
      toolbar={toolbarNode}
      overlays={overlayNode}
    />
  );
}

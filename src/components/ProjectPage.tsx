import { lazy, Suspense, useState, useCallback, useEffect, useMemo, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import type {
  Project,
  Task,
  ProjectMcpStatus,
  BrowserStatus,
} from "../types";
import { SessionPanel } from "./SessionPanel";
import type { FileViewerHandle } from "./FileViewer";
import { ProjectRail } from "./ProjectRail";
import { RightToolbar } from "./RightToolbar";
import type { ShellTerminalPanelHandle } from "./ShellTerminalPanel";
import { ErrorBoundary } from "./ErrorBoundary";
import { MarkdownLinkProvider } from "./markdown/MarkdownLinkContext";
import { ChatPageV2 } from "./chat-page-v2";
import { useProjectPanels } from "../hooks/useProjectPanels";
import {
  ProjectMainArea,
  ProjectRightPanelHost,
  ProjectWorkbench,
  ProjectWorkspaceLayout,
} from "./project/ProjectWorkspaceLayout";

const FileExplorer = lazy(() =>
  import("./FileExplorer").then((module) => ({ default: module.FileExplorer })),
);
const FileViewer = lazy(() =>
  import("./FileViewer").then((module) => ({ default: module.FileViewer })),
);
const GitChanges = lazy(() =>
  import("./GitChanges").then((module) => ({ default: module.GitChanges })),
);
const GitHistory = lazy(() =>
  import("./GitHistory").then((module) => ({ default: module.GitHistory })),
);
const GitDiffViewer = lazy(() =>
  import("./GitDiffViewer").then((module) => ({ default: module.GitDiffViewer })),
);
const ShellTerminalPanel = lazy(() =>
  import("./ShellTerminalPanel").then((module) => ({ default: module.ShellTerminalPanel })),
);
const BrowserPanel = lazy(() =>
  import("./BrowserPanel").then((module) => ({ default: module.BrowserPanel })),
);
const McpStatusDialog = lazy(() =>
  import("./McpStatusDialog").then((module) => ({ default: module.McpStatusDialog })),
);
const AppSettingsDialog = lazy(() =>
  import("./AppSettingsDialog").then((module) => ({ default: module.AppSettingsDialog })),
);
const BrowserDock = lazy(() =>
  import("./BrowserDock").then((module) => ({ default: module.BrowserDock })),
);

function LazyPaneFallback({ label = "加载中..." }: { label?: string }) {
  return (
    <div
      style={{
        flex: 1,
        minWidth: 0,
        minHeight: 0,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        color: "var(--text-muted)",
        fontSize: 13,
        background: "var(--bg-panel)",
      }}
    >
      {label}
    </div>
  );
}

export function ProjectPage({
  project,
  visible = true,
  allProjects = [],
  tasks,
  onBack,
  onSwitchProject,
  onOpen,
}: {
  project: Project;
  visible?: boolean;
  allProjects?: Project[];
  tasks: Task[];
  onBack: () => void;
  onSwitchProject: (project: Project) => void;
  onOpen: () => void;
}) {
  const {
    rightPanel,
    editorWorkbenchVisible,
    openFiles,
    activeFileTabId,
    openDiff,
    rightPanelWidth,
    browserPanelExpanded,
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
    handleToggleBrowserPanelExpanded,
    handleTerminalResizeStart,
    handleOpenPanel,
  } = useProjectPanels();

  const [showShellTerminal, setShowShellTerminal] = useState(false);
  const [showDispatcherSettings, setShowDispatcherSettings] = useState(false);
  const [showMcpStatus, setShowMcpStatus] = useState(false);
  const [mcpStatus, setMcpStatus] = useState<ProjectMcpStatus | null>(null);
  const [mcpChecking, setMcpChecking] = useState(false);
  const [mcpUpdatingServer, setMcpUpdatingServer] = useState<string | null>(null);
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [editorPaneRatio, setEditorPaneRatio] = useState(0.5);
  const [showSessionWorkbench, setShowSessionWorkbench] = useState(true);
  const [sessionSidebarCollapsed, setSessionSidebarCollapsed] = useState(false);
  const shellRef = useRef<ShellTerminalPanelHandle>(null);
  const workspaceSplitRef = useRef<HTMLDivElement>(null);
  const fileViewerRef = useRef<FileViewerHandle>(null);
  const activeSessionIdRef = useRef(activeSessionId);
  activeSessionIdRef.current = activeSessionId;
  const hasEditorWorkbenchContent = openDiff !== null || openFiles.length > 0;
  const showSessionPane = showSessionWorkbench;
  const showEditorPane = editorWorkbenchVisible && hasEditorWorkbenchContent;
  const workbenchColumnCount = Number(showSessionPane) + Number(showEditorPane);

  const handleSelectSession = useCallback((sessionId: string | null) => {
    setActiveSessionId(sessionId);
    if (sessionId) {
      setShowSessionWorkbench(true);
    }
  }, []);

  const handleEditorPaneResizeStart = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    const container = workspaceSplitRef.current;
    if (!container) return;

    const rect = container.getBoundingClientRect();
    const updateRatio = (clientX: number) => {
      const nextRatio = (rect.right - clientX) / rect.width;
      setEditorPaneRatio(Math.max(0.28, Math.min(0.72, nextRatio)));
    };

    const onMouseMove = (event: MouseEvent) => {
      updateRatio(event.clientX);
    };
    const onMouseUp = () => {
      document.removeEventListener("mousemove", onMouseMove);
      document.removeEventListener("mouseup", onMouseUp);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };

    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
    updateRatio(e.clientX);
    document.addEventListener("mousemove", onMouseMove);
    document.addEventListener("mouseup", onMouseUp);
  }, []);

  useEffect(() => {
    if (!hasEditorWorkbenchContent) {
      setShowSessionWorkbench(true);
    }
  }, [hasEditorWorkbenchContent]);

  const refreshMcpStatus = useCallback(async () => {
    setMcpChecking(true);
    try {
      const nextStatus = await invoke<ProjectMcpStatus>("refresh_project_mcp_status", {
        projectPath: project.path,
      });
      setMcpStatus(nextStatus);
    } catch (error) {
      console.error("refresh_project_mcp_status 失败:", error);
    } finally {
      setMcpChecking(false);
    }
  }, [project.path]);

  const handleToggleMcpServerEnabled = useCallback(
    async (serverName: string, enabled: boolean) => {
      setMcpUpdatingServer(serverName);
      try {
        const nextStatus = await invoke<ProjectMcpStatus>("set_project_mcp_server_enabled", {
          projectPath: project.path,
          serverName,
          enabled,
        });
        setMcpStatus(nextStatus);
      } catch (error) {
        console.error("set_project_mcp_server_enabled 失败:", error);
      } finally {
        setMcpUpdatingServer((current) => (current === serverName ? null : current));
      }
    },
    [project.path],
  );

  useEffect(() => {
    if (!visible) return;
    refreshMcpStatus().catch(console.error);
  }, [refreshMcpStatus, visible]);

  useEffect(() => {
    if (!visible) return;
    const unsub = listen<BrowserStatus>("browser-status", (event) => {
      const { sessionId, state } = event.payload;
      if (
        sessionId === activeSessionIdRef.current &&
        state !== "closed" &&
        state !== "page_closed"
      ) {
        handleOpenPanel("browser");
      }
    });
    return () => {
      unsub.then((fn) => fn()).catch(() => {});
    };
  }, [handleOpenPanel, visible]);

  const [dockedBrowsers, setDockedBrowsers] = useState<
    Map<string, { sessionId: string; url: string | null; state: string }>
  >(new Map());

  useEffect(() => {
    const unsubs = [
      listen<BrowserStatus>("browser-status", (event) => {
        const { sessionId, state, url } = event.payload;
        if (state === "minimized" || state === "page_closed") {
          setDockedBrowsers((prev) => {
            const next = new Map(prev);
            next.set(sessionId, { sessionId, url: url ?? null, state });
            return next;
          });
        } else if (state !== "closed") {
          setDockedBrowsers((prev) => {
            if (!prev.has(sessionId)) return prev;
            const next = new Map(prev);
            next.delete(sessionId);
            return next;
          });
        }
      }),
    ];
    return () => {
      unsubs.forEach((u) => u.then((fn) => fn()).catch(() => {}));
    };
  }, []);

  const handleMinimizeBrowser = useCallback(async () => {
    if (!activeSessionId) return;
    await invoke("browser_minimize", { sessionId: activeSessionId });
    handleTogglePanel("browser");
  }, [activeSessionId, handleTogglePanel]);

  const handleRestoreBrowser = useCallback(
    (sessionId: string) => {
      invoke("browser_restore", { sessionId })
        .then(() => {
          handleOpenPanel("browser");
          setActiveSessionId((prev) => (prev === sessionId ? prev : sessionId));
        })
        .catch(console.error);
    },
    [handleOpenPanel],
  );

  const handleCloseDockedBrowser = useCallback((sessionId: string) => {
    invoke("browser_stop", { sessionId })
      .then(() => {
        setDockedBrowsers((prev) => {
          const next = new Map(prev);
          next.delete(sessionId);
          return next;
        });
      })
      .catch(console.error);
  }, []);

  const handleReopenBrowser = useCallback(async () => {
    if (!activeSessionId) return;
    await invoke("browser_reopen", { sessionId: activeSessionId });
    handleOpenPanel("browser");
  }, [activeSessionId, handleOpenPanel]);

  const dockedSessions = useMemo(
    () => Array.from(dockedBrowsers.values()),
    [dockedBrowsers],
  );

  const handleOpenMarkdownLink = useCallback(
    async (url: string) => {
      if (!activeSessionId) return;
      handleOpenPanel("browser");
      try {
        await invoke("browser_navigate", {
          sessionId: activeSessionId,
          url,
          projectPath: project.path,
        });
      } catch (error) {
        console.error("CloakBrowser 打开链接失败:", error);
      }
    },
    [activeSessionId, handleOpenPanel, project.path],
  );

  const railNode = (
    <ProjectRail
      projects={allProjects}
      allTasks={tasks}
      activeProjectId={project.id}
      sessionSidebarCollapsed={sessionSidebarCollapsed}
      onExpandSessionSidebar={() => setSessionSidebarCollapsed(false)}
      onSwitch={onSwitchProject}
      onOpen={onOpen}
    />
  );

  const sessionPanelNode = !sessionSidebarCollapsed ? (
    <SessionPanel
      project={project}
      activeSessionId={activeSessionId}
      onSelectSession={handleSelectSession}
      onBack={onBack}
      onCollapse={() => setSessionSidebarCollapsed(true)}
    />
  ) : undefined;

  const sessionPaneNode = (
    <ErrorBoundary
      label="会话区"
      fallback={(error, reset) => (
        <div className="ai-error-boundary">
          <div className="ai-error-boundary-icon">⚠</div>
          <div className="ai-error-boundary-title">会话区渲染出错</div>
          <div className="ai-error-boundary-message">{error.message || "未知错误"}</div>
          <div className="ai-error-boundary-actions">
            <button onClick={reset} className="ai-error-boundary-btn">
              重试
            </button>
          </div>
        </div>
      )}
    >
      <Suspense fallback={<LazyPaneFallback label="会话加载中..." />}>
        {activeSessionId ? (
          <MarkdownLinkProvider onOpenUrl={handleOpenMarkdownLink}>
            <ChatPageV2
              sessionId={activeSessionId}
              onSessionChange={handleSelectSession}
              conversationKind="project"
              projectPath={project.path}
              mcpStatus={mcpStatus}
              mcpChecking={mcpChecking}
              onOpenMcpStatus={() => setShowMcpStatus(true)}
              onOpenSettings={() => setShowDispatcherSettings(true)}
              onClosePanel={() => setShowSessionWorkbench(false)}
              embedded
            />
          </MarkdownLinkProvider>
        ) : (
          <div
            style={{
              flex: 1,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              color: "var(--text-muted)",
            }}
          >
            正在创建会话...
          </div>
        )}
      </Suspense>
    </ErrorBoundary>
  );

  const editorPaneNode = (
    <ErrorBoundary
      label="编辑区"
      fallback={(error, reset) => (
        <div className="ai-error-boundary">
          <div className="ai-error-boundary-icon">⚠</div>
          <div className="ai-error-boundary-title">编辑区渲染出错</div>
          <div className="ai-error-boundary-message">{error.message || "未知错误"}</div>
          <div className="ai-error-boundary-actions">
            <button onClick={reset} className="ai-error-boundary-btn">
              重试
            </button>
            <button
              onClick={() => {
                clearFileAndDiff();
                reset();
              }}
              className="ai-error-boundary-btn"
            >
              关闭编辑区
            </button>
          </div>
        </div>
      )}
    >
      <Suspense fallback={<LazyPaneFallback label="编辑器加载中..." />}>
        {openDiff ? (
          openDiff.kind === "file" ? (
            <GitDiffViewer
              projectPath={project.path}
              mode="file"
              filePath={openDiff.filePath}
              staged={openDiff.staged}
              title={openDiff.label}
              onClose={() => setOpenDiff(null)}
            />
          ) : openDiff.kind === "commit-file" ? (
            <GitDiffViewer
              projectPath={project.path}
              mode="commit-file"
              commitHash={openDiff.hash}
              filePath={openDiff.filePath}
              title={openDiff.label}
              onClose={() => setOpenDiff(null)}
            />
          ) : (
            <GitDiffViewer
              projectPath={project.path}
              mode="commit"
              commitHash={openDiff.hash}
              title={openDiff.message}
              onClose={() => setOpenDiff(null)}
            />
          )
        ) : (
          <FileViewer
            ref={fileViewerRef}
            tabs={openFiles}
            activeTabId={activeFileTabId}
            projectPath={project.path}
            onSelectTab={handleFileTabSelect}
            onCloseTab={handleFileTabClose}
            onCloseOtherTabs={handleCloseOtherFileTabs}
            onCloseTabsToRight={handleCloseTabsToRight}
            onCloseAllTabs={handleCloseAllFileTabs}
            onHide={hideEditorWorkbench}
          />
        )}
      </Suspense>
    </ErrorBoundary>
  );

  const emptyWorkbenchNode = (
    <div
      style={{
        minWidth: 0,
        minHeight: 0,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        color: "var(--text-muted)",
        background:
          "linear-gradient(180deg, color-mix(in srgb, var(--bg-card) 72%, transparent), var(--bg-panel))",
      }}
    >
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          gap: 10,
          textAlign: "center",
        }}
      >
        <div style={{ fontSize: 14, color: "var(--text-secondary)" }}>
          当前没有打开的会话面板或文件预览
        </div>
        <button type="button" className="ai-error-boundary-btn" onClick={() => setShowSessionWorkbench(true)}>
          打开会话面板
        </button>
        {hasEditorWorkbenchContent && !showEditorPane && (
          <button type="button" className="ai-error-boundary-btn" onClick={showEditorWorkbench}>
            恢复文件编辑器
          </button>
        )}
      </div>
    </div>
  );

  const workbenchNode = (
    <ProjectWorkbench
      workspaceSplitRef={workspaceSplitRef}
      columnCount={workbenchColumnCount}
      editorPaneRatio={editorPaneRatio}
      showSessionPane={showSessionPane}
      sessionPane={sessionPaneNode}
      showEditorPane={showEditorPane}
      editorPane={editorPaneNode}
      emptyPane={emptyWorkbenchNode}
      onEditorPaneResizeStart={handleEditorPaneResizeStart}
    />
  );

  const shellTerminalNode = showShellTerminal ? (
    <Suspense fallback={null}>
      <ShellTerminalPanel
        ref={shellRef}
        projectPath={project.path}
        projectId={project.id}
        isActive={visible}
        onClose={() => setShowShellTerminal(false)}
        height={terminalHeight}
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

  const rightPanelNode = rightPanel ? (
    <ProjectRightPanelHost onResizeStart={handleRightResizeStart}>
      {rightPanel === "files" && (
        <ErrorBoundary label="文件浏览器">
          <Suspense fallback={<LazyPaneFallback label="文件列表加载中..." />}>
            <FileExplorer
              projectPath={project.path}
              projectName={project.name}
              onFileSelect={handleFileSelect}
              onFileRename={handleFileTreeRename}
              onFileDelete={handleFileTreeDelete}
              openFilePaths={openFiles.map((tab) => tab.path)}
              active={visible}
              width={rightPanelWidth}
            />
          </Suspense>
        </ErrorBoundary>
      )}
      {rightPanel === "git-changes" && (
        <ErrorBoundary label="Git 变更">
          <Suspense fallback={<LazyPaneFallback label="Git 变更加载中..." />}>
            <GitChanges
              projectPath={project.path}
              currentTaskCreatedAt={null}
              onFileSelect={handleDiffFileSelect}
              width={rightPanelWidth}
            />
          </Suspense>
        </ErrorBoundary>
      )}
      {rightPanel === "git-history" && (
        <ErrorBoundary label="Git 历史">
          <Suspense fallback={<LazyPaneFallback label="Git 历史加载中..." />}>
            <GitHistory
              projectPath={project.path}
              onCommitSelect={handleCommitSelect}
              onFileClick={handleCommitFileClick}
              width={rightPanelWidth}
            />
          </Suspense>
        </ErrorBoundary>
      )}
      {rightPanel === "browser" && (
        <ErrorBoundary label="CloakBrowser">
          <Suspense fallback={<LazyPaneFallback label="浏览器加载中..." />}>
            <BrowserPanel
              sessionId={activeSessionId}
              projectPath={project.path}
              width={rightPanelWidth}
              active={visible}
              expanded={browserPanelExpanded}
              onToggleExpanded={handleToggleBrowserPanelExpanded}
              onClose={() => handleTogglePanel("browser")}
              onMinimize={handleMinimizeBrowser}
              onReopen={handleReopenBrowser}
            />
          </Suspense>
        </ErrorBoundary>
      )}
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
    <>
      {showDispatcherSettings && (
        <Suspense fallback={null}>
          <AppSettingsDialog
            initialTab="aha"
            projectId={project.id}
            projectPath={project.path}
            onClose={() => setShowDispatcherSettings(false)}
          />
        </Suspense>
      )}

      {showMcpStatus && (
        <Suspense fallback={null}>
          <McpStatusDialog
            projectPath={project.path}
            status={mcpStatus}
            checking={mcpChecking}
            updatingServer={mcpUpdatingServer}
            onRefresh={() => {
              refreshMcpStatus().catch(console.error);
            }}
            onToggleServerEnabled={(serverName, enabled) => {
              handleToggleMcpServerEnabled(serverName, enabled).catch(console.error);
            }}
            onClose={() => setShowMcpStatus(false)}
          />
        </Suspense>
      )}

      {dockedSessions.length > 0 && (
        <Suspense fallback={null}>
          <BrowserDock
            sessions={dockedSessions}
            onRestore={handleRestoreBrowser}
            onClose={handleCloseDockedBrowser}
          />
        </Suspense>
      )}
    </>
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

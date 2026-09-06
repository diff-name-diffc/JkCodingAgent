import { lazy, Suspense, useCallback, useEffect, useRef } from "react";
import type { McpStatus, Project } from "../../types";
import type { useProjectPanels } from "../../hooks/useProjectPanels";
import type { WorkspaceBudget } from "./workspace-budget";
import { ChatPageV2 } from "../chat-page-v2";
import { ErrorBoundary } from "../ErrorBoundary";
import { MarkdownLinkProvider } from "../markdown/MarkdownLinkContext";
import { ProjectWorkbench } from "./ProjectWorkspaceLayout";
import { ProjectLazyPaneFallback } from "./ProjectLazyPaneFallback";

const FileViewer = lazy(() =>
  import("../FileViewer").then((module) => ({ default: module.FileViewer })),
);
const GitDiffViewer = lazy(() =>
  import("../GitDiffViewer").then((module) => ({ default: module.GitDiffViewer })),
);

type ProjectPanelController = ReturnType<typeof useProjectPanels>;

interface ProjectWorkbenchContentProps {
  project: Project;
  activeSessionId: string | null;
  mcpStatus: McpStatus | null;
  mcpChecking: boolean;
  panels: ProjectPanelController;
  sessionWorkbenchVisible: boolean;
  onSessionWorkbenchVisibleChange: (visible: boolean) => void;
  onSelectSession: (sessionId: string | null) => void;
  onOpenMarkdownLink: (url: string) => void | Promise<void>;
  onOpenMcpStatus: () => void;
  onOpenSettings: () => void;
  /** 工作区是否可见（保活隐藏时为 false），透传给 portal 覆盖层门控。 */
  workspaceVisible: boolean;
  /** 空间预算（UI-04）：双栏/单栏与像素宽由纯函数模块决定。 */
  budget: WorkspaceBudget;
  editorPaneRatio: number;
  onEditorPaneRatioChange: (ratio: number) => void;
}

export function ProjectWorkbenchContent({
  project,
  activeSessionId,
  mcpStatus,
  mcpChecking,
  panels,
  sessionWorkbenchVisible,
  onSessionWorkbenchVisibleChange,
  onSelectSession,
  onOpenMarkdownLink,
  onOpenMcpStatus,
  onOpenSettings,
  workspaceVisible,
  budget,
  editorPaneRatio,
  onEditorPaneRatioChange,
}: ProjectWorkbenchContentProps) {
  const workspaceSplitRef = useRef<HTMLDivElement>(null);
  const hasEditorContent = panels.openDiff !== null || panels.openFiles.length > 0;
  const editorRequested = panels.editorWorkbenchVisible && hasEditorContent;
  // 预算降级为单栏时：会话面板优先；用户主动收起会话后编辑区独占。
  const dual = budget.dualPane && sessionWorkbenchVisible && editorRequested;
  const showSessionPane = dual || sessionWorkbenchVisible;
  const showEditorPane = dual || (!sessionWorkbenchVisible && editorRequested);
  const columnCount = Number(showSessionPane) + Number(showEditorPane);

  const handleEditorPaneResizeStart = useCallback(
    (event: React.MouseEvent) => {
      event.preventDefault();
      const container = workspaceSplitRef.current;
      if (!container) return;

      const rect = container.getBoundingClientRect();
      const updateRatio = (clientX: number) => {
        // 占比偏好自由写入；像素下限由 resolveWorkspaceBudget 夹取。
        const nextRatio = (rect.right - clientX) / rect.width;
        onEditorPaneRatioChange(Math.max(0, Math.min(1, nextRatio)));
      };
      const onMouseMove = (moveEvent: MouseEvent) => updateRatio(moveEvent.clientX);
      const onMouseUp = () => {
        document.removeEventListener("mousemove", onMouseMove);
        document.removeEventListener("mouseup", onMouseUp);
        document.body.style.cursor = "";
        document.body.style.userSelect = "";
      };

      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";
      updateRatio(event.clientX);
      document.addEventListener("mousemove", onMouseMove);
      document.addEventListener("mouseup", onMouseUp);
    },
    [onEditorPaneRatioChange],
  );

  const handleEditorPaneResizeKey = useCallback(
    (event: React.KeyboardEvent) => {
      const step = event.shiftKey ? 0.1 : 0.02;
      if (event.key === "ArrowLeft") {
        event.preventDefault();
        onEditorPaneRatioChange(Math.max(0, Math.min(1, editorPaneRatio + step)));
      } else if (event.key === "ArrowRight") {
        event.preventDefault();
        onEditorPaneRatioChange(Math.max(0, Math.min(1, editorPaneRatio - step)));
      }
    },
    [editorPaneRatio, onEditorPaneRatioChange],
  );

  useEffect(() => {
    if (!hasEditorContent) onSessionWorkbenchVisibleChange(true);
  }, [hasEditorContent, onSessionWorkbenchVisibleChange]);

  const sessionPane = (
    <ErrorBoundary
      label="会话区"
      fallback={(error, reset) => (
        <div className="ai-error-boundary">
          <div className="ai-error-boundary-icon">⚠</div>
          <div className="ai-error-boundary-title">会话区渲染出错</div>
          <div className="ai-error-boundary-message">{error.message || "未知错误"}</div>
          <div className="ai-error-boundary-actions">
            <button type="button" onClick={reset} className="ai-error-boundary-btn">
              重试
            </button>
          </div>
        </div>
      )}
    >
      <Suspense fallback={<ProjectLazyPaneFallback label="会话加载中..." />}>
        {activeSessionId ? (
          <MarkdownLinkProvider onOpenUrl={onOpenMarkdownLink}>
            <ChatPageV2
              sessionId={activeSessionId}
              onSessionChange={onSelectSession}
              conversationKind="project"
              projectPath={project.path}
              mcpStatus={mcpStatus}
              mcpChecking={mcpChecking}
              onOpenMcpStatus={onOpenMcpStatus}
              onOpenSettings={onOpenSettings}
              onClosePanel={() => onSessionWorkbenchVisibleChange(false)}
              embedded
              workspaceVisible={workspaceVisible}
            />
          </MarkdownLinkProvider>
        ) : (
          <ProjectLazyPaneFallback label="正在创建会话..." />
        )}
      </Suspense>
    </ErrorBoundary>
  );

  const editorPane = (
    <ErrorBoundary
      label="编辑区"
      fallback={(error, reset) => (
        <div className="ai-error-boundary">
          <div className="ai-error-boundary-icon">⚠</div>
          <div className="ai-error-boundary-title">编辑区渲染出错</div>
          <div className="ai-error-boundary-message">{error.message || "未知错误"}</div>
          <div className="ai-error-boundary-actions">
            <button type="button" onClick={reset} className="ai-error-boundary-btn">
              重试
            </button>
            <button
              type="button"
              onClick={() => {
                panels.clearFileAndDiff();
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
      <Suspense fallback={<ProjectLazyPaneFallback label="编辑器加载中..." />}>
        {panels.activeEditorTab?.kind === "diff" ? (
          panels.activeEditorTab.diff.kind === "file" ? (
            <GitDiffViewer
              projectPath={project.path}
              mode="file"
              filePath={panels.activeEditorTab.diff.filePath}
              staged={panels.activeEditorTab.diff.staged}
              title={panels.activeEditorTab.diff.label}
              onClose={panels.handleCloseActiveDiff}
            />
          ) : panels.activeEditorTab.diff.kind === "commit-file" ? (
            <GitDiffViewer
              projectPath={project.path}
              mode="commit-file"
              commitHash={panels.activeEditorTab.diff.hash}
              filePath={panels.activeEditorTab.diff.filePath}
              title={panels.activeEditorTab.diff.label}
              onClose={panels.handleCloseActiveDiff}
            />
          ) : (
            <GitDiffViewer
              projectPath={project.path}
              mode="commit"
              commitHash={panels.activeEditorTab.diff.hash}
              title={panels.activeEditorTab.diff.message}
              onClose={panels.handleCloseActiveDiff}
            />
          )
        ) : (
          <FileViewer
            tabs={panels.openFiles}
            activeTabId={panels.activeFileTabId}
            projectPath={project.path}
            onSelectTab={panels.handleFileTabSelect}
            onCloseTab={panels.handleFileTabClose}
            onCloseOtherTabs={panels.handleCloseOtherFileTabs}
            onCloseTabsToRight={panels.handleCloseTabsToRight}
            onCloseAllTabs={panels.handleCloseAllFileTabs}
            onHide={panels.hideEditorWorkbench}
          />
        )}
      </Suspense>
    </ErrorBoundary>
  );

  const emptyPane = (
    <div
      className="flex min-h-0 min-w-0 items-center justify-center"
      style={{ background: "var(--bg-panel)", color: "var(--text-muted)" }}
    >
      <div className="flex flex-col items-center gap-2.5 text-center">
        <div className="text-sm" style={{ color: "var(--text-secondary)" }}>
          当前没有打开的会话面板或文件预览
        </div>
        <button
          type="button"
          className="ai-error-boundary-btn"
          onClick={() => onSessionWorkbenchVisibleChange(true)}
        >
          打开会话面板
        </button>
        {hasEditorContent && !showEditorPane && (
          <button
            type="button"
            className="ai-error-boundary-btn"
            onClick={panels.showEditorWorkbench}
          >
            恢复文件编辑器
          </button>
        )}
      </div>
    </div>
  );

  return (
    <ProjectWorkbench
      workspaceSplitRef={workspaceSplitRef}
      columnCount={columnCount}
      gridTemplateColumns={
        dual
          ? `${budget.chatWidth}px ${budget.splitterWidth}px ${budget.editorWidth}px`
          : "minmax(0, 1fr)"
      }
      showSessionPane={showSessionPane}
      sessionPane={sessionPane}
      showEditorPane={showEditorPane}
      editorPane={editorPane}
      emptyPane={emptyPane}
      onEditorPaneResizeStart={handleEditorPaneResizeStart}
      onEditorPaneResizeKey={handleEditorPaneResizeKey}
      onEditorPaneResizeDoubleClick={() => onEditorPaneRatioChange(0.5)}
    />
  );
}

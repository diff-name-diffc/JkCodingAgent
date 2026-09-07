import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";
import type { McpStatus, Project } from "../../types";
import type { useProjectPanels } from "../../hooks/useProjectPanels";
import type { GraphTab } from "../../hooks/useGraphTabSync";
import { splitDualPaneWidths, type WorkspaceBudget } from "./workspace-budget";
import { nextSplitterValue, splitterKeyDelta } from "../../lib/splitter-step";
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
const GraphPanel = lazy(() =>
  import("../graph/GraphPanel").then((module) => ({ default: module.GraphPanel })),
);
const BrowserPanel = lazy(() =>
  import("../browser/BrowserPanel").then((module) => ({ default: module.BrowserPanel })),
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
  /** 工作区是否可见（保活隐藏时为 false）：门控聊天侧常驻副作用与
   * 主区标签视图（执行图等）的快捷键/自适应响应。 */
  workspaceVisible: boolean;
  /** 空间预算（UI-04）：双栏/单栏与像素宽由纯函数模块决定。 */
  budget: WorkspaceBudget;
  editorPaneRatio: number;
  onEditorPaneRatioChange: (ratio: number) => void;
  /** 关闭执行图标签（UI-13；由 useGraphTabSync 提供，同步清除打开意图）。 */
  onCloseGraphTab: (tab: GraphTab) => void;
  /** 扩大/还原主区（收起/恢复会话 pane），执行图与浏览器标签共用。 */
  onExpandMainArea: () => void;
  /** 浏览器窗口最小化/重开（UI-18：来自 useBrowserSessionDock 的命令层）。 */
  onMinimizeBrowser?: () => void | Promise<void>;
  onReopenBrowser?: () => void | Promise<void>;
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
  onCloseGraphTab,
  onExpandMainArea,
  onMinimizeBrowser,
  onReopenBrowser,
}: ProjectWorkbenchContentProps) {
  const workspaceSplitRef = useRef<HTMLDivElement>(null);
  // UI-13：编辑区内容判定收敛到 useProjectPanels 单一派生值（含 file/diff/graph 标签）。
  const hasEditorContent = panels.hasEditorContent;
  const editorRequested = panels.editorWorkbenchVisible && hasEditorContent;
  // 先取出图标签（JSX 闭包内联合类型收窄不保留）。
  const activeGraphTab =
    panels.activeEditorTab?.kind === "graph" ? panels.activeEditorTab : null;
  // 预算降级为单栏时：会话面板优先；用户主动收起会话后编辑区独占。
  const dual = budget.dualPane && sessionWorkbenchVisible && editorRequested;
  const showSessionPane = dual || sessionWorkbenchVisible;
  const showEditorPane = dual || (!sessionWorkbenchVisible && editorRequested);
  const columnCount = Number(showSessionPane) + Number(showEditorPane);
  // 拖拽隔离（UI-24a-4）：拖拽期只更新本组件本地 dragRatio，mouseup 一次性
  // 写回持久化偏好（详见 handleEditorPaneResizeStart 注释）。
  const [dragRatio, setDragRatio] = useState<number | null>(null);
  // 拖拽期双栏宽度本地重算（与 resolveWorkspaceBudget 同一钳制口径）；
  // 非拖拽期直接用预算输出。
  const dualWidths =
    dual && dragRatio !== null
      ? splitDualPaneWidths(budget.chatWidth, budget.editorWidth, dragRatio)
      : { chatWidth: budget.chatWidth, editorWidth: budget.editorWidth };

  // 拖拽隔离（UI-24a-4）：对齐 ContextNav 范式——拖拽期只更新本地 dragRatio
  // （双栏宽度经 splitDualPaneWidths 本地重算），mouseup 一次性写回持久化偏好。
  // 旧实现每 mousemove 调 onEditorPaneRatioChange → zustand persist 同步写
  // localStorage + ProjectPage 全树重渲染。
  const dragCleanupRef = useRef<(() => void) | null>(null);
  // 拖拽中途卸载（切项目/关工作区）：兜底执行 mouseup 语义（提交并摘监听），
  // 避免 dragRatio 悬挂与 document 监听泄漏。
  useEffect(
    () => () => {
      dragCleanupRef.current?.();
    },
    [],
  );

  const handleEditorPaneResizeStart = useCallback(
    (event: React.MouseEvent) => {
      event.preventDefault();
      const container = workspaceSplitRef.current;
      if (!container) return;

      const rect = container.getBoundingClientRect();
      // 占比自由取值（0..1）；像素下限由 splitDualPaneWidths / 预算同一口径夹取。
      const ratioAt = (clientX: number) =>
        Math.max(0, Math.min(1, (rect.right - clientX) / rect.width));
      let latest = ratioAt(event.clientX);
      const onMouseMove = (moveEvent: MouseEvent) => {
        latest = ratioAt(moveEvent.clientX);
        setDragRatio(latest);
      };
      const onMouseUp = () => {
        dragCleanupRef.current = null;
        document.removeEventListener("mousemove", onMouseMove);
        document.removeEventListener("mouseup", onMouseUp);
        document.body.style.cursor = "";
        document.body.style.userSelect = "";
        setDragRatio(null);
        onEditorPaneRatioChange(latest);
      };
      dragCleanupRef.current = onMouseUp;

      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";
      setDragRatio(latest);
      document.addEventListener("mousemove", onMouseMove);
      document.addEventListener("mouseup", onMouseUp);
    },
    [onEditorPaneRatioChange],
  );

  const handleEditorPaneResizeKey = useCallback(
    (event: React.KeyboardEvent) => {
      // 语义收敛到 lib/splitter-step（UI-23c）：占比从右缘测量，ArrowLeft=增大
      // （invert）；小步 0.02 / Shift 大步 0.1，与既有行为一致。
      const delta = splitterKeyDelta(event.key, "vertical", true);
      if (delta === null) return;
      event.preventDefault();
      onEditorPaneRatioChange(
        nextSplitterValue({
          current: editorPaneRatio,
          delta,
          mode: "ratio",
          shift: event.shiftKey,
          min: 0,
          max: 1,
        }),
      );
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
              projectId={project.id}
              projectName={project.name}
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
        {activeGraphTab ? (
          <GraphPanel
            planId={activeGraphTab.planId}
            sessionId={activeGraphTab.sessionId}
            active={workspaceVisible && showEditorPane}
            onClose={() => onCloseGraphTab(activeGraphTab)}
            onExpandMainArea={onExpandMainArea}
            mainAreaExpanded={!sessionWorkbenchVisible}
          />
        ) : panels.activeEditorTab?.kind === "browser" ? (
          /* 浏览器预览标签（UI-18）：内容跟随活动会话；关标签=隐藏面板不停进程；
             扩大按钮与执行图同语义（收起会话 pane 占满主区）。 */
          <BrowserPanel
            sessionId={activeSessionId}
            projectPath={project.path}
            active={workspaceVisible && showEditorPane}
            expanded={!sessionWorkbenchVisible}
            onToggleExpanded={onExpandMainArea}
            onClose={panels.handleCloseBrowserTab}
            onMinimize={onMinimizeBrowser}
            onReopen={onReopenBrowser}
          />
        ) : panels.activeEditorTab?.kind === "diff" ? (
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
          ? `${dualWidths.chatWidth}px ${budget.splitterWidth}px ${dualWidths.editorWidth}px`
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
      editorPaneRatio={dragRatio ?? editorPaneRatio}
    />
  );
}

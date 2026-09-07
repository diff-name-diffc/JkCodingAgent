import type React from "react";
import type { CSSProperties, ReactNode, RefObject } from "react";

interface ProjectWorkspaceLayoutProps {
  visible: boolean;
  rootStyle: CSSProperties;
  rail: ReactNode;
  sessionPanel?: ReactNode;
  main: ReactNode;
  overlays?: ReactNode;
}

/**
 * 项目工作区外壳（UI-18 收敛后）：rail / 上下文导航 / 主区 / 覆盖层四槽。
 * 旧右面板槽（浏览器）与旧右工具栏槽（UI-07 废弃）均已移除——浏览器迁入
 * 主区标签，工具栏由 main 内 StatusDockBar 取代。
 */
export function ProjectWorkspaceLayout({
  visible,
  rootStyle,
  rail,
  sessionPanel,
  main,
  overlays,
}: ProjectWorkspaceLayoutProps) {
  return (
    <div
      className="ai-project-shell"
      style={{
        ...rootStyle,
        position: "absolute",
        inset: 0,
        visibility: visible ? "visible" : "hidden",
        pointerEvents: visible ? "auto" : "none",
        zIndex: visible ? 1 : 0,
      }}
    >
      {rail}
      {sessionPanel}
      {main}
      {overlays}
    </div>
  );
}

interface ProjectMainAreaProps {
  workbench: ReactNode;
  subProcessTabs?: ReactNode;
  /** 终端 dock（UI-19）：隐藏 = 组件内 CSS display:none（PTY 保活）；
   * 仅面板头部「结束会话」卸载组件并 kill shell，槽位本身不承载生命周期。 */
  shellTerminal?: ReactNode;
  /** 底部 24px 状态/dock 条（UI-07 StatusDockBar）。 */
  statusDock?: ReactNode;
  mainStyle: CSSProperties;
}

export function ProjectMainArea({
  workbench,
  subProcessTabs,
  shellTerminal,
  statusDock,
  mainStyle,
}: ProjectMainAreaProps) {
  return (
    <div className="ai-project-main" style={{ ...mainStyle, flexDirection: "column" }}>
      <div
        className="ai-project-workbench-frame"
        style={{
          flex: 1,
          display: "flex",
          flexDirection: "column",
          overflow: "hidden",
          minHeight: 0,
          position: "relative",
        }}
      >
        {workbench}
      </div>
      {subProcessTabs}
      {shellTerminal}
      {statusDock}
    </div>
  );
}

interface ProjectWorkbenchProps {
  workspaceSplitRef: RefObject<HTMLDivElement | null>;
  columnCount: number;
  /** 由空间预算纯函数算出的 grid 列定义（双栏为像素三列，单栏 1fr）。 */
  gridTemplateColumns: string;
  showSessionPane: boolean;
  sessionPane?: ReactNode;
  showEditorPane: boolean;
  editorPane?: ReactNode;
  emptyPane?: ReactNode;
  onEditorPaneResizeStart: (event: React.MouseEvent<HTMLDivElement>) => void;
  onEditorPaneResizeKey?: (event: React.KeyboardEvent<HTMLDivElement>) => void;
  onEditorPaneResizeDoubleClick?: () => void;
  /** 编辑区占比（0..1），用于分隔条 aria-valuenow 百分比读数（UI-23c）。 */
  editorPaneRatio?: number;
}

export function ProjectWorkbench({
  workspaceSplitRef,
  columnCount,
  gridTemplateColumns,
  showSessionPane,
  sessionPane,
  showEditorPane,
  editorPane,
  emptyPane,
  onEditorPaneResizeStart,
  onEditorPaneResizeKey,
  onEditorPaneResizeDoubleClick,
  editorPaneRatio,
}: ProjectWorkbenchProps) {
  return (
    <div
      ref={workspaceSplitRef}
      className="ai-project-workbench-grid"
      style={{
        flex: 1,
        minHeight: 0,
        display: "grid",
        gridTemplateColumns,
        overflow: "hidden",
        background: "var(--bg-panel)",
      }}
    >
      {showSessionPane && <div className="ai-project-chat-pane ai-project-workbench-pane">{sessionPane}</div>}

      {columnCount === 2 && (
        <div
          className="ai-splitter ai-project-splitter"
          role="separator"
          aria-orientation="vertical"
          aria-label="拖拽或方向键调整会话与编辑区宽度，双击恢复默认"
          tabIndex={0}
          aria-valuenow={
            editorPaneRatio !== undefined
              ? Math.round(Math.max(0, Math.min(1, editorPaneRatio)) * 100)
              : undefined
          }
          aria-valuemin={0}
          aria-valuemax={100}
          onMouseDown={onEditorPaneResizeStart}
          onKeyDown={onEditorPaneResizeKey}
          onDoubleClick={onEditorPaneResizeDoubleClick}
          style={{
            width: 8,
            cursor: "col-resize",
          }}
        />
      )}

      {showEditorPane && (
        <div
          className="ai-project-editor-pane ai-project-workbench-pane"
          style={{
            borderLeft: columnCount === 2 ? "1px solid var(--border-dim)" : "none",
            background: "var(--bg-panel)",
          }}
        >
          {editorPane}
        </div>
      )}

      {columnCount === 0 && emptyPane}
    </div>
  );
}

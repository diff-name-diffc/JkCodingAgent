import type React from "react";
import type { CSSProperties, ReactNode, RefObject } from "react";

interface ProjectWorkspaceLayoutProps {
  visible: boolean;
  rootStyle: CSSProperties;
  rail: ReactNode;
  sessionPanel?: ReactNode;
  main: ReactNode;
  rightPanel?: ReactNode;
  toolbar: ReactNode;
  overlays?: ReactNode;
}

export function ProjectWorkspaceLayout({
  visible,
  rootStyle,
  rail,
  sessionPanel,
  main,
  rightPanel,
  toolbar,
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
      {rightPanel}
      {toolbar}
      {overlays}
    </div>
  );
}

interface ProjectMainAreaProps {
  workbench: ReactNode;
  subProcessTabs?: ReactNode;
  shellTerminal?: ReactNode;
  mainStyle: CSSProperties;
}

export function ProjectMainArea({
  workbench,
  subProcessTabs,
  shellTerminal,
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
          onMouseDown={onEditorPaneResizeStart}
          onKeyDown={onEditorPaneResizeKey}
          onDoubleClick={onEditorPaneResizeDoubleClick}
          style={{
            width: 8,
            cursor: "col-resize",
            background:
              "linear-gradient(180deg, transparent, color-mix(in srgb, var(--accent) 14%, var(--border-dim)), transparent)",
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

interface ProjectRightPanelHostProps {
  onResizeStart: (event: React.MouseEvent<HTMLDivElement>) => void;
  onResizeKey?: (event: React.KeyboardEvent<HTMLDivElement>) => void;
  onResizeDoubleClick?: () => void;
  children: ReactNode;
}

export function ProjectRightPanelHost({
  onResizeStart,
  onResizeKey,
  onResizeDoubleClick,
  children,
}: ProjectRightPanelHostProps) {
  return (
    <div className="ai-project-right-panel" style={{ position: "relative", display: "flex", flexShrink: 0 }}>
      <div
        className="ai-splitter ai-project-right-resizer"
        role="separator"
        aria-orientation="vertical"
        aria-label="拖拽或方向键调整右栏宽度，双击恢复默认"
        tabIndex={0}
        onMouseDown={onResizeStart}
        onKeyDown={onResizeKey}
        onDoubleClick={onResizeDoubleClick}
        style={{
          position: "absolute",
          left: 0,
          top: 0,
          bottom: 0,
          width: 5,
          cursor: "col-resize",
          zIndex: 10,
        }}
      />
      {children}
    </div>
  );
}

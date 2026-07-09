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
      className="ai-project-shell ai-migrated-project"
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
  editorPaneRatio: number;
  showSessionPane: boolean;
  sessionPane?: ReactNode;
  showEditorPane: boolean;
  editorPane?: ReactNode;
  emptyPane?: ReactNode;
  onEditorPaneResizeStart: (event: React.MouseEvent<HTMLDivElement>) => void;
}

export function ProjectWorkbench({
  workspaceSplitRef,
  columnCount,
  editorPaneRatio,
  showSessionPane,
  sessionPane,
  showEditorPane,
  editorPane,
  emptyPane,
  onEditorPaneResizeStart,
}: ProjectWorkbenchProps) {
  return (
    <div
      ref={workspaceSplitRef}
      className="ai-project-workbench-grid"
      style={{
        flex: 1,
        minHeight: 0,
        display: "grid",
        gridTemplateColumns:
          columnCount === 2
            ? `minmax(0, calc(${(1 - editorPaneRatio) * 100}% - 4px)) 8px minmax(0, calc(${editorPaneRatio * 100}% - 4px))`
            : "minmax(0, 1fr)",
        overflow: "hidden",
        background: "var(--bg-panel)",
      }}
    >
      {showSessionPane && <div className="ai-project-chat-pane ai-project-workbench-pane">{sessionPane}</div>}

      {columnCount === 2 && (
        <div
          className="ai-splitter ai-project-splitter"
          onMouseDown={onEditorPaneResizeStart}
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
  children: ReactNode;
}

export function ProjectRightPanelHost({
  onResizeStart,
  children,
}: ProjectRightPanelHostProps) {
  return (
    <div className="ai-project-right-panel" style={{ position: "relative", display: "flex", flexShrink: 0 }}>
      <div
        className="ai-splitter ai-project-right-resizer"
        onMouseDown={onResizeStart}
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

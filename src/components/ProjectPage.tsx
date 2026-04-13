import { useMemo, useState, useCallback, useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import type {
  Project,
  Task,
  AgentType,
  PermissionMode,
  ThemeMode,
  SubProcess,
} from "../types";
import { cleanTerminalOutput } from "../utils/ansiStrip";
import { FileExplorer } from "./FileExplorer";
import { SessionPanel } from "./SessionPanel";
import { FileViewer } from "./FileViewer";
import { GitChanges } from "./GitChanges";
import { GitHistory } from "./GitHistory";
import { GitDiffViewer } from "./GitDiffViewer";
import { ProjectRail } from "./ProjectRail";
import { SettingsDialog } from "./SettingsDialog";
import { RightToolbar } from "./RightToolbar";
import { ShellTerminalPanel, type ShellTerminalPanelHandle } from "./ShellTerminalPanel";
import { DispatcherChat, type DispatcherChatHandle } from "./DispatcherChat";
import { SubProcessTabs } from "./SubProcessTabs";
import { AppSettingsDialog } from "./AppSettingsDialog";
import { ErrorBoundary } from "./ErrorBoundary";
import { useProjectPanels } from "../hooks/useProjectPanels";
import s from "../styles";

export function ProjectPage({
  project,
  visible = true,
  allProjects = [],
  tasks,
  getTaskRestoreState,
  onSubmitTask,
  onInput,
  onResize,
  onRegisterTerminal,
  onTerminalReady,
  onSnapshot,
  onBack,
  onSwitchProject,
  onOpen,
  isDark,
  themeMode,
  systemPrefersDark,
  onThemeModeChange,
  onToggleTheme,
}: {
  project: Project;
  visible?: boolean;
  allProjects?: Project[];
  tasks: Task[];
  getTaskRestoreState: (taskId: string) => { initialData?: string; initialSnapshot?: string };
  onSubmitTask: (task: {
    prompt: string;
    agent: AgentType;
    permissionMode: PermissionMode;
    images: string[];
    immediate: boolean;
    hidden?: boolean;
    dispatcherDispatchId?: string;
  }) => void;
  onInput: (taskId: string, data: string) => void;
  onResize: (taskId: string, cols: number, rows: number) => void;
  onRegisterTerminal: (
    taskId: string,
    writeFn: ((data: string, callback?: () => void) => void) | null,
  ) => number;
  onTerminalReady: (taskId: string, generation: number) => void;
  onSnapshot: (taskId: string, snapshot: string) => void;
  onBack: () => void;
  onSwitchProject: (project: Project) => void;
  onOpen: () => void;
  isDark: boolean;
  themeMode: ThemeMode;
  systemPrefersDark: boolean;
  onThemeModeChange: (mode: ThemeMode) => void;
  onToggleTheme: () => void;
}) {
  const {
    rightPanel,
    openFiles,
    activeFilePath,
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
    handleDiffFileSelect,
    handleCommitSelect,
    handleCommitFileClick,
    clearFileAndDiff,
    handleRightResizeStart,
    handleTerminalResizeStart,
  } = useProjectPanels();

  const [showShellTerminal, setShowShellTerminal] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [showDispatcherSettings, setShowDispatcherSettings] = useState(false);
  const [subProcesses, setSubProcesses] = useState<SubProcess[]>([]);
  const [activeSubTabId, setActiveSubTabId] = useState<string | null>(null);
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [subTerminalHeight, setSubTerminalHeight] = useState(500);
  /** Maps subprocess id → real task id for terminal routing */
  const [subProcessTaskMap, setSubProcessTaskMap] = useState<Record<string, string>>({});
  const shellRef = useRef<ShellTerminalPanelHandle>(null);
  const pendingCmdRef = useRef<string | null>(null);
  const dispatcherChatRef = useRef<DispatcherChatHandle>(null);
  /** Track dispatchId → subprocess id for correlating dispatch results */
  const pendingDispatchRef = useRef<Map<string, { spId: string; dispatchId: string }>>(new Map());
  /** Track subprocess task_ids that were voluntarily exited via /exit */
  const exitedSubprocessesRef = useRef<Set<string>>(new Set());

  const projectTasks = useMemo(
    () => tasks.filter((t) => t.projectId === project.id),
    [tasks, project.id],
  );

  const handleRunMakeTarget = useCallback(
    (target: string) => {
      const cmd = `make ${target}\n`;
      if (showShellTerminal && shellRef.current) {
        shellRef.current.sendCommand(cmd);
      } else {
        pendingCmdRef.current = cmd;
        setShowShellTerminal(true);
      }
    },
    [showShellTerminal],
  );

  const handleShellReady = useCallback(() => {
    if (pendingCmdRef.current) {
      shellRef.current?.sendCommand(pendingCmdRef.current);
      pendingCmdRef.current = null;
    }
  }, []);

  // ── Dispatcher sub-process handlers ──
  const handleDispatchApproved = useCallback(
    (dispatchId: string, description: string, permissionMode: string) => {
      const spId = `sp_${Date.now()}`;
      const sp: SubProcess = {
        id: spId,
        dispatchId,
        agent: "claude",
        description,
        status: "running",
        startedAt: Date.now(),
      };
      setSubProcesses((prev) => [...prev, sp]);
      setActiveSubTabId(spId);

      // Track pending dispatch for result injection
      pendingDispatchRef.current.set(spId, { spId, dispatchId });

      // Start a real Claude task via PTY infrastructure.
      // The task will be created by App.tsx and its id returned via tasks prop update.
      // We detect the new task by matching prompt content.
      onSubmitTask({
        prompt: description,
        agent: "claude",
        permissionMode: (permissionMode as PermissionMode) || "full_access",
        images: [],
        immediate: true,
        hidden: true,
        dispatcherDispatchId: dispatchId,
      });
    },
    [onSubmitTask],
  );

  // Detect newly created tasks and map them to subprocesses
  const prevTaskCountRef = useRef(projectTasks.length);
  useEffect(() => {
    const prevCount = prevTaskCountRef.current;
    prevTaskCountRef.current = projectTasks.length;

    if (projectTasks.length > prevCount) {
      // A new task was added — check if there's a pending subprocess
      const newestTask = projectTasks.reduce((a, b) =>
        a.createdAt > b.createdAt ? a : b
      );
      // Find the subprocess that doesn't have a task mapped yet
      const unmappedSp = subProcesses.find(
        (sp) => sp.status === "running" && !subProcessTaskMap[sp.id]
      );
      if (unmappedSp && newestTask) {
        setSubProcessTaskMap((prev) => ({ ...prev, [unmappedSp.id]: newestTask.id }));
      }
    }
  }, [projectTasks.length]); // eslint-disable-line react-hooks/exhaustive-deps

  // Monitor task completion for subprocesses → inject result back
  useEffect(() => {
    const unsub = listen<{ task_id: string; status: string }>(
      "task-status",
      (e) => {
        const { task_id, status } = e.payload;
        if (status !== "done" && status !== "failed" && status !== "cancelled") return;

        // Find subprocess for this task
        const spEntry = Object.entries(subProcessTaskMap).find(
          ([, tid]) => tid === task_id
        );
        if (!spEntry) return;
        const [spId] = spEntry;

        // Update subprocess status
        const isDone = status === "done";
        setSubProcesses((prev) =>
          prev.map((sp) =>
            sp.id === spId
              ? { ...sp, status: isDone ? "done" : "failed" }
              : sp
          )
        );

        // If this subprocess was voluntarily exited via /exit, skip result injection
        if (exitedSubprocessesRef.current.has(task_id)) {
          exitedSubprocessesRef.current.delete(task_id);
          pendingDispatchRef.current.delete(spId);
          return;
        }

        // Capture terminal output and inject back to Dispatcher
        const pending = pendingDispatchRef.current.get(spId);
        if (pending) {
          pendingDispatchRef.current.delete(spId);

          // Get terminal content from restore state (buffer)
          const restoreState = getTaskRestoreState(task_id);
          const rawOutput = (restoreState.initialSnapshot || "") + (restoreState.initialData || "");
          const cleaned = cleanTerminalOutput(rawOutput);

          const resultText = isDone
            ? `Claude 子任务完成。\n\n终端输出：\n${cleaned}`
            : `Claude 子任务失败 (status: ${status})。\n\n终端输出：\n${cleaned}`;

          // Continue the Dispatcher Agent with the result via DispatcherChat ref
          // This ensures streaming events are handled by the chat UI
          dispatcherChatRef.current?.continueWithResult(resultText);
        }
      }
    );
    return () => { unsub.then((fn) => fn()); };
  }, [subProcessTaskMap, project.path, getTaskRestoreState]);

  // Listen for dispatcher-subprocess-idle events (stream idle detection)
  useEffect(() => {
    const unsub = listen<{ task_id: string; output: string }>(
      "dispatcher-subprocess-idle",
      (e) => {
        const { task_id, output } = e.payload;

        // Find subprocess for this task
        const spEntry = Object.entries(subProcessTaskMap).find(
          ([, tid]) => tid === task_id
        );
        if (!spEntry) return;

        // Clean and inject output into Dispatcher Agent
        const cleaned = cleanTerminalOutput(output);
        const resultText = `Claude 当前轮次执行完成（流空闲检测）。\n\n终端输出：\n${cleaned}`;
        dispatcherChatRef.current?.continueWithResult(resultText);
      }
    );
    return () => { unsub.then((fn) => fn()); };
  }, [subProcessTaskMap]);

  const handleDispatchRejected = useCallback((_dispatchId: string) => {
    // No-op for now, the agent already recorded the rejection
  }, []);

  // Handle DispatchContinue: send text to active subprocess terminal
  const handleDispatchContinue = useCallback(
    (text: string) => {
      // Find the most recently active subprocess
      const activeSp = subProcesses.find((sp) => sp.status === "running");
      if (!activeSp) return;
      const taskId = subProcessTaskMap[activeSp.id];
      if (!taskId) return;

      invoke("dispatcher_send_to_subprocess", { taskId, text: text + "\n" }).catch(console.error);
    },
    [subProcesses, subProcessTaskMap],
  );

  // Handle DispatchExit: send /exit to active subprocess terminal
  const handleDispatchExit = useCallback(
    (_reason: string) => {
      const activeSp = subProcesses.find((sp) => sp.status === "running");
      if (!activeSp) return;
      const taskId = subProcessTaskMap[activeSp.id];
      if (!taskId) return;

      // Mark as voluntarily exited so we skip result injection on task-status
      exitedSubprocessesRef.current.add(taskId);
      invoke("dispatcher_exit_subprocess", { taskId }).catch(console.error);
    },
    [subProcesses, subProcessTaskMap],
  );

  const handleCloseSubTab = useCallback((id: string) => {
    setSubProcesses((prev) => prev.filter((sp) => sp.id !== id));
    setActiveSubTabId((prev) => (prev === id ? null : prev));
  }, []);

  const handleSubTerminalResizeStart = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    const startY = e.clientY;
    const startHeight = subTerminalHeight;

    const onMouseMove = (ev: MouseEvent) => {
      const delta = startY - ev.clientY;
      setSubTerminalHeight(Math.max(120, Math.min(600, startHeight + delta)));
    };
    const onMouseUp = () => {
      document.removeEventListener("mousemove", onMouseMove);
      document.removeEventListener("mouseup", onMouseUp);
    };
    document.addEventListener("mousemove", onMouseMove);
    document.addEventListener("mouseup", onMouseUp);
  }, [subTerminalHeight]);

  return (
    <div
      style={{
        ...s.projectBody,
        position: "absolute",
        inset: 0,
        visibility: visible ? "visible" : "hidden",
        pointerEvents: visible ? "auto" : "none",
        zIndex: visible ? 1 : 0,
      }}
    >
      <ProjectRail
        projects={allProjects}
        allTasks={tasks}
        activeProjectId={project.id}
        onSwitch={onSwitchProject}
        onOpen={onOpen}
      />
      <SessionPanel
        project={project}
        activeSessionId={activeSessionId}
        onSelectSession={setActiveSessionId}
        onBack={onBack}
        isDark={isDark}
        themeMode={themeMode}
        systemPrefersDark={systemPrefersDark}
        onThemeModeChange={onThemeModeChange}
        onToggleTheme={onToggleTheme}
      />
      <div style={{ ...s.mainContent, flexDirection: "column" }}>
        <div
          style={{
            flex: 1,
            display: "flex",
            flexDirection: "column",
            overflow: "hidden",
            minHeight: 0,
            position: "relative",
          }}
        >
          {/* Foreground: file viewer, diff, or dispatcher session */}
          <ErrorBoundary
            label="主内容区"
            fallback={(error, reset) => (
              <div style={s.errorBoundaryWrap}>
                <div style={s.errorBoundaryIcon}>⚠</div>
                <div style={s.errorBoundaryTitle}>内容区渲染出错</div>
                <div style={s.errorBoundaryMessage}>{error.message || "未知错误"}</div>
                <div style={s.errorBoundaryActions}>
                  <button onClick={reset} style={s.errorBoundaryBtn}>
                    重试
                  </button>
                  <button
                    onClick={() => {
                      clearFileAndDiff();
                      reset();
                    }}
                    style={s.errorBoundaryBtn}
                  >
                    返回任务视图
                  </button>
                </div>
              </div>
            )}
          >
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
            ) : openFiles.length > 0 ? (
              <FileViewer
                tabs={openFiles}
                activeFilePath={activeFilePath}
                projectPath={project.path}
                onSelectTab={handleFileTabSelect}
                onCloseTab={handleFileTabClose}
                onCloseOtherTabs={handleCloseOtherFileTabs}
                onCloseTabsToRight={handleCloseTabsToRight}
                onCloseAllTabs={handleCloseAllFileTabs}
                isDark={isDark}
                onRunMakeTarget={handleRunMakeTarget}
              />
            ) : activeSessionId ? (
              <DispatcherChat
                ref={dispatcherChatRef}
                sessionId={activeSessionId}
                projectPath={project.path}
                subProcesses={subProcesses}
                onDispatchApproved={handleDispatchApproved}
                onDispatchRejected={handleDispatchRejected}
                onDispatchContinue={handleDispatchContinue}
                onDispatchExit={handleDispatchExit}
                onOpenSettings={() => setShowDispatcherSettings(true)}
              />
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
          </ErrorBoundary>
        </div>
        {/* Sub-process terminal tabs */}
        {subProcesses.length > 0 && (
          <SubProcessTabs
            subProcesses={subProcesses}
            activeTabId={activeSubTabId}
            onSelectTab={setActiveSubTabId}
            onCloseTab={handleCloseSubTab}
            height={subTerminalHeight}
            onResizeStart={handleSubTerminalResizeStart}
            isDark={isDark}
            onInput={onInput}
            onResize={onResize}
            onRegisterTerminal={onRegisterTerminal}
            onTerminalReady={onTerminalReady}
            onSnapshot={onSnapshot}
            getRestoreState={getTaskRestoreState}
            subProcessTaskMap={subProcessTaskMap}
          />
        )}
        {showShellTerminal && (
          <ShellTerminalPanel
            ref={shellRef}
            projectPath={project.path}
            projectId={project.id}
            isActive={visible}
            onClose={() => setShowShellTerminal(false)}
            isDark={isDark}
            onReady={handleShellReady}
            height={terminalHeight}
            onResizeStart={handleTerminalResizeStart}
          />
        )}
      </div>

      {rightPanel && (
        <div style={{ position: "relative", display: "flex", flexShrink: 0 }}>
          <div
            onMouseDown={handleRightResizeStart}
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
          {rightPanel === "files" && (
            <ErrorBoundary label="文件浏览器">
              <FileExplorer
                projectPath={project.path}
                projectName={project.name}
                onFileSelect={handleFileSelect}
                isDark={isDark}
                active={visible}
                width={rightPanelWidth}
              />
            </ErrorBoundary>
          )}
          {rightPanel === "git-changes" && (
            <ErrorBoundary label="Git 变更">
              <GitChanges
                projectPath={project.path}
                currentTaskCreatedAt={null}
                onFileSelect={handleDiffFileSelect}
                width={rightPanelWidth}
              />
            </ErrorBoundary>
          )}
          {rightPanel === "git-history" && (
            <ErrorBoundary label="Git 历史">
              <GitHistory
                projectPath={project.path}
                onCommitSelect={handleCommitSelect}
                onFileClick={handleCommitFileClick}
                width={rightPanelWidth}
              />
            </ErrorBoundary>
          )}
        </div>
      )}

      <RightToolbar
        activePanel={rightPanel}
        onToggle={handleTogglePanel}
        terminalActive={showShellTerminal}
        onToggleTerminal={() => setShowShellTerminal((v) => !v)}
        onOpenSettings={() => setShowSettings(true)}
      />

      {showSettings && (
        <SettingsDialog projectPath={project.path} onClose={() => setShowSettings(false)} />
      )}

      {showDispatcherSettings && (
        <AppSettingsDialog
          isDark={isDark}
          themeMode={themeMode}
          systemPrefersDark={systemPrefersDark}
          onThemeModeChange={onThemeModeChange}
          initialTab="aha"
          onClose={() => setShowDispatcherSettings(false)}
        />
      )}
    </div>
  );
}

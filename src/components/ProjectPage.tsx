import { useState, useCallback, useEffect, useMemo, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import type {
  Project,
  Task,
  AgentType,
  PermissionMode,
  ProjectMcpStatus,
  ThemeMode,
  SubProcess,
  DispatchFeedbackState,
} from "../types";
import { cleanTerminalOutput } from "../utils/ansiStrip";
import { FileExplorer } from "./FileExplorer";
import { SessionPanel } from "./SessionPanel";
import { FileViewer } from "./FileViewer";
import { GitChanges } from "./GitChanges";
import { GitHistory } from "./GitHistory";
import { GitDiffViewer } from "./GitDiffViewer";
import { ProjectRail } from "./ProjectRail";
import { RightToolbar } from "./RightToolbar";
import { ShellTerminalPanel, type ShellTerminalPanelHandle } from "./ShellTerminalPanel";
import { DispatcherChat, type DispatcherChatHandle } from "./DispatcherChat";
import { McpStatusDialog } from "./McpStatusDialog";
import { SubProcessTabs } from "./SubProcessTabs";
import { AppSettingsDialog } from "./AppSettingsDialog";
import { ErrorBoundary } from "./ErrorBoundary";
import { useProjectPanels } from "../hooks/useProjectPanels";
import s from "../styles";

function getSubProcessAgentLabel(agent: AgentType): string {
  return agent === "claude" ? "Claude" : "Codex";
}

function getSubProcessRouteKey(sessionId: string, agent: AgentType): string {
  return `${sessionId}:${agent}`;
}

function createSubProcessId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return `sp_${crypto.randomUUID()}`;
  }
  return `sp_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`;
}

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
  }) => string;
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
    editorWorkbenchVisible,
    openFiles,
    activeFileTabId,
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
  } = useProjectPanels();

  const [showShellTerminal, setShowShellTerminal] = useState(false);
  const [showDispatcherSettings, setShowDispatcherSettings] = useState(false);
  const [showMcpStatus, setShowMcpStatus] = useState(false);
  const [mcpStatus, setMcpStatus] = useState<ProjectMcpStatus | null>(null);
  const [mcpChecking, setMcpChecking] = useState(false);
  const [mcpUpdatingServer, setMcpUpdatingServer] = useState<string | null>(null);
  const [subProcesses, setSubProcesses] = useState<SubProcess[]>([]);
  const [activeSubTabIdBySession, setActiveSubTabIdBySession] = useState<
    Record<string, string | null>
  >({});
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [subTerminalHeight, setSubTerminalHeight] = useState(500);
  const [editorPaneRatio, setEditorPaneRatio] = useState(0.5);
  const [showSessionWorkbench, setShowSessionWorkbench] = useState(true);
  /** Maps subprocess id → real task id for terminal routing */
  const [subProcessTaskMap, setSubProcessTaskMap] = useState<Record<string, string>>({});
  const shellRef = useRef<ShellTerminalPanelHandle>(null);
  const workspaceSplitRef = useRef<HTMLDivElement>(null);
  const dispatcherChatRef = useRef<DispatcherChatHandle>(null);
  /** Track subprocess id → owning dispatcher session for result injection */
  const pendingDispatchRef = useRef<
    Map<string, { spId: string; dispatchId: string; sessionId: string }>
  >(new Map());
  /** Track the interactive subprocess to continue/exit for each dispatcher session + agent */
  const interactiveSubProcessRef = useRef<Map<string, string>>(new Map());
  /** Track subprocess task_ids that were voluntarily exited via /exit */
  const exitedSubprocessesRef = useRef<Set<string>>(new Set());
  /** Track task_ids whose current round has already been injected via idle detection */
  const idleInjectedTaskIdsRef = useRef<Set<string>>(new Set());
  /** Track task_ids that have already reached a terminal state to block late idle events */
  const closedSubprocessTaskIdsRef = useRef<Set<string>>(new Set());
  const visibleSubProcesses = useMemo(
    () =>
      activeSessionId
        ? subProcesses.filter((subProcess) => subProcess.sessionId === activeSessionId)
        : [],
    [activeSessionId, subProcesses],
  );
  const activeVisibleSubTabId = activeSessionId
    ? (activeSubTabIdBySession[activeSessionId] ?? null)
    : null;
  const hasEditorWorkbenchContent = openDiff !== null || openFiles.length > 0;
  const showSessionPane = showSessionWorkbench;
  const showEditorPane = editorWorkbenchVisible && hasEditorWorkbenchContent;
  const workbenchColumnCount = Number(showSessionPane) + Number(showEditorPane);

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

  // ── Dispatcher sub-process handlers ──
  const handleDispatchApproved = useCallback(
    (
      dispatchId: string,
      agent: AgentType,
      description: string,
      permissionMode: string,
      sessionId: string,
    ) => {
      const spId = createSubProcessId();
      const sp: SubProcess = {
        id: spId,
        dispatchId,
        sessionId,
        agent,
        description,
        status: "running",
        startedAt: Date.now(),
      };
      setSubProcesses((prev) => [...prev, sp]);
      setActiveSubTabIdBySession((prev) => ({ ...prev, [sessionId]: spId }));
      interactiveSubProcessRef.current.set(getSubProcessRouteKey(sessionId, agent), spId);

      // Track pending dispatch for result injection
      pendingDispatchRef.current.set(spId, { spId, dispatchId, sessionId });

      const taskId = onSubmitTask({
        prompt: description,
        agent,
        permissionMode: (permissionMode as PermissionMode) || "full_access",
        images: [],
        immediate: true,
        hidden: true,
        dispatcherDispatchId: dispatchId,
      });
      idleInjectedTaskIdsRef.current.delete(taskId);
      closedSubprocessTaskIdsRef.current.delete(taskId);
      setSubProcessTaskMap((prev) => ({ ...prev, [spId]: taskId }));
      invoke("dispatcher_register_subprocess", {
        workspaceId: sessionId,
        taskId,
        dispatchId,
        agent,
        description,
      }).catch(console.error);
    },
    [onSubmitTask],
  );

  // Monitor task completion for subprocesses → inject result back
  useEffect(() => {
    const unsub = listen<{ task_id: string; status: string }>("task-status", (e) => {
      const { task_id, status } = e.payload;
      if (status !== "done" && status !== "failed" && status !== "cancelled") return;

      // Find subprocess for this task
      const spEntry = Object.entries(subProcessTaskMap).find(([, tid]) => tid === task_id);
      if (!spEntry) return;
      const [spId] = spEntry;
      const alreadyInjectedByIdle = idleInjectedTaskIdsRef.current.has(task_id);
      closedSubprocessTaskIdsRef.current.add(task_id);
      invoke("dispatcher_mark_subprocess_finished", { taskId: task_id }).catch(console.error);
      const targetSubProcess = subProcesses.find((sp) => sp.id === spId);
      const routeKey = targetSubProcess
        ? getSubProcessRouteKey(targetSubProcess.sessionId, targetSubProcess.agent)
        : null;

      // Update subprocess status
      const isDone = status === "done";
      setSubProcesses((prev) =>
        prev.map((sp) => (sp.id === spId ? { ...sp, status: isDone ? "done" : "failed" } : sp)),
      );

      // If this subprocess was voluntarily exited via /exit, skip result injection
      if (exitedSubprocessesRef.current.has(task_id)) {
        exitedSubprocessesRef.current.delete(task_id);
        pendingDispatchRef.current.delete(spId);
        idleInjectedTaskIdsRef.current.delete(task_id);
        if (routeKey && interactiveSubProcessRef.current.get(routeKey) === spId) {
          interactiveSubProcessRef.current.delete(routeKey);
        }
        return;
      }

      // Capture terminal output and inject back to Dispatcher
      const pending = pendingDispatchRef.current.get(spId);
      if (!pending || alreadyInjectedByIdle) {
        pendingDispatchRef.current.delete(spId);
        idleInjectedTaskIdsRef.current.delete(task_id);
        if (routeKey && interactiveSubProcessRef.current.get(routeKey) === spId) {
          interactiveSubProcessRef.current.delete(routeKey);
        }
        return;
      }

      pendingDispatchRef.current.delete(spId);
      idleInjectedTaskIdsRef.current.delete(task_id);

      // Get terminal content from restore state (buffer)
      const restoreState = getTaskRestoreState(task_id);
      const rawOutput = (restoreState.initialSnapshot || "") + (restoreState.initialData || "");
      const cleaned = cleanTerminalOutput(rawOutput);
      const agentLabel = getSubProcessAgentLabel(targetSubProcess?.agent ?? "claude");

      const dispatchState: DispatchFeedbackState =
        status === "done"
          ? "process_done"
          : status === "cancelled"
            ? "process_cancelled"
            : "process_failed";
      const resultText = isDone
        ? `${agentLabel} 子进程已退出，本轮执行已结束。\n\n终端输出：\n${cleaned}`
        : `${agentLabel} 子进程已结束 (status: ${status})。\n\n终端输出：\n${cleaned}`;

      if (routeKey && interactiveSubProcessRef.current.get(routeKey) === spId) {
        interactiveSubProcessRef.current.delete(routeKey);
      }

      dispatcherChatRef.current?.continueWithResult(resultText, dispatchState, pending.sessionId);
    });
    return () => {
      unsub.then((fn) => fn());
    };
  }, [subProcessTaskMap, subProcesses, getTaskRestoreState]);

  // 监听 session watcher 发出的“当前轮次完成”信号，允许在同一子进程内继续注入。
  useEffect(() => {
    const unsub = listen<{ task_id: string; output: string }>("dispatcher-subprocess-idle", (e) => {
      const { task_id, output } = e.payload;
      if (closedSubprocessTaskIdsRef.current.has(task_id)) return;
      if (exitedSubprocessesRef.current.has(task_id)) return;
      if (idleInjectedTaskIdsRef.current.has(task_id)) return;

      // Find subprocess for this task
      const spEntry = Object.entries(subProcessTaskMap).find(([, tid]) => tid === task_id);
      if (!spEntry) return;

      // Clean and inject output into Dispatcher Agent
      const cleaned = cleanTerminalOutput(output);
      const [spId] = spEntry;
      const targetSubProcess = subProcesses.find((sp) => sp.id === spId);
      const agent = targetSubProcess?.agent ?? "claude";
      const agentLabel = getSubProcessAgentLabel(agent);
      const resultText = `${agentLabel} 当前轮次已完成，子进程仍在运行，可继续注入后续指令。\n\n终端输出：\n${cleaned}`;
      const sessionId =
        pendingDispatchRef.current.get(spId)?.sessionId ??
        subProcesses.find((sp) => sp.id === spId)?.sessionId;
      if (!sessionId) return;
      pendingDispatchRef.current.delete(spId);
      idleInjectedTaskIdsRef.current.add(task_id);
      interactiveSubProcessRef.current.set(getSubProcessRouteKey(sessionId, agent), spId);
      invoke("dispatcher_mark_subprocess_round_completed", { taskId: task_id }).catch(
        console.error,
      );
      dispatcherChatRef.current?.continueWithResult(resultText, "round_completed", sessionId);
    });
    return () => {
      unsub.then((fn) => fn());
    };
  }, [subProcessTaskMap, subProcesses]);

  const handleDispatchRejected = useCallback((_dispatchId: string) => {
    // No-op for now, the agent already recorded the rejection
  }, []);

  // Handle DispatchContinue: send text to active subprocess terminal
  const handleDispatchContinue = useCallback(
    (agent: AgentType, text: string, sessionId: string) => {
      const routeKey = getSubProcessRouteKey(sessionId, agent);
      const preferredSpId = interactiveSubProcessRef.current.get(routeKey);
      const activeSp =
        (preferredSpId
          ? subProcesses.filter((sp) => sp.id === preferredSpId)
          : [...subProcesses].reverse()
        ).find(
          (sp) => sp.status === "running" && sp.sessionId === sessionId && sp.agent === agent,
        ) ??
        [...subProcesses]
          .reverse()
          .find(
            (sp) => sp.status === "running" && sp.sessionId === sessionId && sp.agent === agent,
          );
      if (!activeSp) return;
      const taskId = subProcessTaskMap[activeSp.id];
      if (!taskId) return;

      interactiveSubProcessRef.current.set(routeKey, activeSp.id);
      idleInjectedTaskIdsRef.current.delete(taskId);
      invoke("dispatcher_mark_subprocess_running", { taskId }).catch(console.error);
      const submittedText = text.replace(/(?:\r?\n)+$/, "");
      invoke("dispatcher_send_to_subprocess", { taskId, text: submittedText }).catch(console.error);
    },
    [subProcesses, subProcessTaskMap],
  );

  // Handle DispatchExit: send /exit to active subprocess terminal
  const handleDispatchExit = useCallback(
    (agent: AgentType, _reason: string, sessionId: string) => {
      const routeKey = getSubProcessRouteKey(sessionId, agent);
      const preferredSpId = interactiveSubProcessRef.current.get(routeKey);
      const activeSp =
        (preferredSpId
          ? subProcesses.filter((sp) => sp.id === preferredSpId)
          : [...subProcesses].reverse()
        ).find(
          (sp) => sp.status === "running" && sp.sessionId === sessionId && sp.agent === agent,
        ) ??
        [...subProcesses]
          .reverse()
          .find(
            (sp) => sp.status === "running" && sp.sessionId === sessionId && sp.agent === agent,
          );
      if (!activeSp) return;
      const taskId = subProcessTaskMap[activeSp.id];
      if (!taskId) return;

      interactiveSubProcessRef.current.set(routeKey, activeSp.id);
      // Mark as voluntarily exited so we skip result injection on task-status
      exitedSubprocessesRef.current.add(taskId);
      closedSubprocessTaskIdsRef.current.add(taskId);
      invoke("dispatcher_exit_subprocess", { taskId }).catch(console.error);
    },
    [subProcesses, subProcessTaskMap],
  );

  const handleCloseSubTab = useCallback(
    (id: string) => {
      const targetSubProcess = subProcesses.find((sp) => sp.id === id);
      setSubProcesses((prev) => prev.filter((sp) => sp.id !== id));
      if (targetSubProcess) {
        const routeKey = getSubProcessRouteKey(targetSubProcess.sessionId, targetSubProcess.agent);
        if (interactiveSubProcessRef.current.get(routeKey) === id) {
          interactiveSubProcessRef.current.delete(routeKey);
        }
      }
      setActiveSubTabIdBySession((prev) => {
        if (!targetSubProcess) return prev;
        if (prev[targetSubProcess.sessionId] !== id) return prev;
        return { ...prev, [targetSubProcess.sessionId]: null };
      });
    },
    [subProcesses],
  );

  const handleSubTerminalResizeStart = useCallback(
    (e: React.MouseEvent) => {
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
    },
    [subTerminalHeight],
  );

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
        onSelectSession={(sessionId) => {
          setActiveSessionId(sessionId);
          if (sessionId) {
            setShowSessionWorkbench(true);
          }
        }}
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
          <div
            ref={workspaceSplitRef}
            style={{
              flex: 1,
              minHeight: 0,
              display: "grid",
              gridTemplateColumns:
                workbenchColumnCount === 2
                  ? `minmax(0, calc(${(1 - editorPaneRatio) * 100}% - 4px)) 8px minmax(0, calc(${editorPaneRatio * 100}% - 4px))`
                  : "minmax(0, 1fr)",
              overflow: "hidden",
              background: "var(--bg-panel)",
            }}
          >
            {showSessionPane && (
              <div
                style={{
                  minWidth: 0,
                  minHeight: 0,
                  display: "flex",
                  overflow: "hidden",
                }}
              >
                <ErrorBoundary
                  label="会话区"
                  fallback={(error, reset) => (
                    <div style={s.errorBoundaryWrap}>
                      <div style={s.errorBoundaryIcon}>⚠</div>
                      <div style={s.errorBoundaryTitle}>会话区渲染出错</div>
                      <div style={s.errorBoundaryMessage}>{error.message || "未知错误"}</div>
                      <div style={s.errorBoundaryActions}>
                        <button onClick={reset} style={s.errorBoundaryBtn}>
                          重试
                        </button>
                      </div>
                    </div>
                  )}
                >
                  {activeSessionId ? (
                    <DispatcherChat
                      ref={dispatcherChatRef}
                      sessionId={activeSessionId}
                      projectPath={project.path}
                      mcpStatus={mcpStatus}
                      mcpChecking={mcpChecking}
                      layoutMode={showEditorPane ? "split" : "single"}
                      subProcesses={subProcesses}
                      onDispatchApproved={handleDispatchApproved}
                      onDispatchRejected={handleDispatchRejected}
                      onDispatchContinue={handleDispatchContinue}
                      onDispatchExit={handleDispatchExit}
                      onOpenMcpStatus={() => setShowMcpStatus(true)}
                      onOpenSettings={() => setShowDispatcherSettings(true)}
                      onClosePanel={() => setShowSessionWorkbench(false)}
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
            )}

            {workbenchColumnCount === 2 && (
              <div
                onMouseDown={handleEditorPaneResizeStart}
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
                style={{
                  minWidth: 0,
                  minHeight: 0,
                  display: "flex",
                  overflow: "hidden",
                  borderLeft:
                    workbenchColumnCount === 2 ? "1px solid var(--border-dim)" : "none",
                  background: "var(--bg-panel)",
                }}
              >
                <ErrorBoundary
                  label="编辑区"
                  fallback={(error, reset) => (
                    <div style={s.errorBoundaryWrap}>
                      <div style={s.errorBoundaryIcon}>⚠</div>
                      <div style={s.errorBoundaryTitle}>编辑区渲染出错</div>
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
                          关闭编辑区
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
                  ) : (
                    <FileViewer
                      tabs={openFiles}
                      activeTabId={activeFileTabId}
                      projectPath={project.path}
                      onSelectTab={handleFileTabSelect}
                      onCloseTab={handleFileTabClose}
                      onCloseOtherTabs={handleCloseOtherFileTabs}
                      onCloseTabsToRight={handleCloseTabsToRight}
                      onCloseAllTabs={handleCloseAllFileTabs}
                      onHide={hideEditorWorkbench}
                      isDark={isDark}
                    />
                  )}
                </ErrorBoundary>
              </div>
            )}

            {workbenchColumnCount === 0 && (
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
                  <button
                    type="button"
                    style={s.errorBoundaryBtn}
                    onClick={() => setShowSessionWorkbench(true)}
                  >
                    打开会话面板
                  </button>
                  {hasEditorWorkbenchContent && !showEditorPane && (
                    <button type="button" style={s.errorBoundaryBtn} onClick={showEditorWorkbench}>
                      恢复文件编辑器
                    </button>
                  )}
                </div>
              </div>
            )}
          </div>
        </div>
        {/* Sub-process terminal tabs */}
        {visibleSubProcesses.length > 0 && (
          <SubProcessTabs
            subProcesses={visibleSubProcesses}
            activeTabId={activeVisibleSubTabId}
            onSelectTab={(id) => {
              if (!activeSessionId) return;
              setActiveSubTabIdBySession((prev) => ({
                ...prev,
                [activeSessionId]: prev[activeSessionId] === id ? null : id,
              }));
            }}
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
                onFileRename={handleFileTreeRename}
                onFileDelete={handleFileTreeDelete}
                openFilePaths={openFiles.map((tab) => tab.path)}
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
      />

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

      {showMcpStatus && (
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
      )}
    </div>
  );
}

import { lazy, Suspense, useState, useCallback, useEffect, useMemo, useRef } from "react";
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
  TaskStatus,
  DispatcherSessionRuntimeState,
  BrowserStatus,
} from "../types";
import { cleanTerminalOutput } from "../utils/ansiStrip";
import { SessionPanel } from "./SessionPanel";
import type { FileViewerHandle } from "./FileViewer";
import { ProjectRail } from "./ProjectRail";
import { RightToolbar } from "./RightToolbar";
import type { ShellTerminalPanelHandle } from "./ShellTerminalPanel";
import type { DispatcherChatHandle } from "./DispatcherChat";
import { ErrorBoundary } from "./ErrorBoundary";
import { MarkdownLinkProvider } from "./markdown/MarkdownLinkContext";
import { useProjectPanels } from "../hooks/useProjectPanels";
import { getPathBasename } from "../utils/filePaths";
import s from "../styles";

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
const DispatcherChat = lazy(() =>
  import("./DispatcherChat").then((module) => ({ default: module.DispatcherChat })),
);
const McpStatusDialog = lazy(() =>
  import("./McpStatusDialog").then((module) => ({ default: module.McpStatusDialog })),
);
const SubProcessTabs = lazy(() =>
  import("./SubProcessTabs").then((module) => ({ default: module.SubProcessTabs })),
);
const AppSettingsDialog = lazy(() =>
  import("./AppSettingsDialog").then((module) => ({ default: module.AppSettingsDialog })),
);
const BrowserDock = lazy(() =>
  import("./BrowserDock").then((module) => ({ default: module.BrowserDock })),
);

function getSubProcessAgentLabel(agent: AgentType): string {
  return agent === "claude" ? "Claude" : "Codex";
}

function getSubProcessRouteKey(sessionId: string, agent: AgentType): string {
  return `${sessionId}:${agent}`;
}

function isDispatcherTask(task: Task, projectId: string): boolean {
  return (
    task.projectId === projectId &&
    !!task.dispatcherSessionId &&
    !!task.dispatcherDispatchId &&
    !!task.dispatcherDescription
  );
}

function mapTaskStatusToSubProcessStatus(status: TaskStatus): SubProcess["status"] {
  switch (status) {
    case "stopped":
      return "stopped";
    case "done":
      return "done";
    case "failed":
    case "cancelled":
      return "failed";
    default:
      return "running";
  }
}

function toSubProcess(task: Task): SubProcess | null {
  if (!task.dispatcherSessionId || !task.dispatcherDispatchId || !task.dispatcherDescription) {
    return null;
  }

  return {
    id: task.id,
    dispatchId: task.dispatcherDispatchId,
    sessionId: task.dispatcherSessionId,
    agent: task.agent,
    description: task.dispatcherDescription,
    status: mapTaskStatusToSubProcessStatus(task.status),
    startedAt: task.createdAt,
    failureReason: task.failureReason,
  };
}

function toErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

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
  getTaskRestoreState,
  onStartSubProcess,
  onInput,
  onResize,
  onRegisterTerminal,
  onTerminalReady,
  onSnapshot,
  onRetainTaskBuffers,
  onReleaseTaskBuffers,
  onRemoveTaskBuffers,
  onStopTask,
  onResumeTask,
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
  onStartSubProcess: (task: {
    prompt: string;
    agent: AgentType;
    permissionMode: PermissionMode;
    dispatcherDispatchId: string;
    dispatcherSessionId: string;
    dispatcherDescription: string;
  }) => string;
  onStopTask: (taskId: string) => Promise<void>;
  onResumeTask: (task: Task) => Promise<void>;
  onInput: (taskId: string, data: string) => void;
  onResize: (taskId: string, cols: number, rows: number) => void;
  onRegisterTerminal: (
    taskId: string,
    writeFn: ((data: string, callback?: () => void) => void) | null,
  ) => number;
  onTerminalReady: (taskId: string, generation: number) => void;
  onSnapshot: (taskId: string, snapshot: string) => void;
  onRetainTaskBuffers: (taskIds: string[]) => void;
  onReleaseTaskBuffers: (taskIds: string[]) => void;
  onRemoveTaskBuffers: (taskIds: string[]) => void;
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
  const [dismissedSubProcessIds, setDismissedSubProcessIds] = useState<Set<string>>(new Set());
  const [activeSubTabIdBySession, setActiveSubTabIdBySession] = useState<
    Record<string, string | null>
  >({});
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [subTerminalHeight, setSubTerminalHeight] = useState(500);
  const [editorPaneRatio, setEditorPaneRatio] = useState(0.5);
  const [showSessionWorkbench, setShowSessionWorkbench] = useState(true);
  const [sessionSidebarCollapsed, setSessionSidebarCollapsed] = useState(false);
  const shellRef = useRef<ShellTerminalPanelHandle>(null);
  const workspaceSplitRef = useRef<HTMLDivElement>(null);
  const dispatcherChatRef = useRef<DispatcherChatHandle>(null);
  const fileViewerRef = useRef<FileViewerHandle>(null);
  const activeSessionIdRef = useRef(activeSessionId);
  activeSessionIdRef.current = activeSessionId;
  /** Track task_id → owning dispatcher session for result injection */
  const pendingDispatchRef = useRef<
    Map<string, { taskId: string; dispatchId: string; sessionId: string }>
  >(new Map());
  /** Track the interactive subprocess to continue/exit for each dispatcher session + agent */
  const interactiveSubProcessRef = useRef<Map<string, string>>(new Map());
  /** Track subprocess task_ids that were voluntarily exited via /exit */
  const exitedSubprocessesRef = useRef<Set<string>>(new Set());
  /** Track task_ids whose current round has already been injected via idle detection */
  const idleInjectedTaskIdsRef = useRef<Set<string>>(new Set());
  /** Track task_ids that have already reached a terminal state to block late idle events */
  const closedSubprocessTaskIdsRef = useRef<Set<string>>(new Set());
  const allSubProcesses = useMemo(
    () =>
      tasks
        .filter((task) => isDispatcherTask(task, project.id))
        .map((task) => toSubProcess(task))
        .filter((subProcess): subProcess is SubProcess => !!subProcess)
        .sort((left, right) => right.startedAt - left.startedAt),
    [project.id, tasks],
  );
  const allSubProcessesRef = useRef(allSubProcesses);
  allSubProcessesRef.current = allSubProcesses;
  const subProcesses = useMemo(
    () =>
      allSubProcesses
        .filter((subProcess) => !dismissedSubProcessIds.has(subProcess.id))
        .sort((left, right) => right.startedAt - left.startedAt),
    [allSubProcesses, dismissedSubProcessIds],
  );
  const subprocessRunningSessionIds = useMemo(() => {
    const runningSessionIds = new Set<string>();
    for (const subProcess of allSubProcesses) {
      if (subProcess.status === "running") {
        runningSessionIds.add(subProcess.sessionId);
      }
    }
    return runningSessionIds;
  }, [allSubProcesses]);
  const activeVisibleSubTabId = activeSessionId
    ? (activeSubTabIdBySession[activeSessionId] ?? null)
    : null;
  const activeSessionSubProcesses = useMemo(
    () =>
      activeSessionId
        ? subProcesses.filter((subProcess) => subProcess.sessionId === activeSessionId)
        : [],
    [activeSessionId, subProcesses],
  );
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

  useEffect(() => {
    if (!activeSessionId || activeSessionSubProcesses.length === 0) return;

    const currentActiveId = activeSubTabIdBySession[activeSessionId];
    if (currentActiveId && activeSessionSubProcesses.some((subProcess) => subProcess.id === currentActiveId)) {
      return;
    }

    setActiveSubTabIdBySession((prev) => ({
      ...prev,
      [activeSessionId]: activeSessionSubProcesses[0].id,
    }));
  }, [activeSessionId, activeSessionSubProcesses, activeSubTabIdBySession]);

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

  // ── Dispatcher sub-process handlers ──
  const handleDispatchApproved = useCallback(
    (
      dispatchId: string,
      agent: AgentType,
      description: string,
      taskPrompt: string,
      permissionMode: string,
      sessionId: string,
    ) => {
      const taskId = onStartSubProcess({
        prompt: taskPrompt,
        agent,
        permissionMode: (permissionMode as PermissionMode) || "full_access",
        dispatcherDispatchId: dispatchId,
        dispatcherSessionId: sessionId,
        dispatcherDescription: description,
      });
      setDismissedSubProcessIds((prev) => {
        if (!prev.has(taskId)) return prev;
        const next = new Set(prev);
        next.delete(taskId);
        return next;
      });
      setActiveSubTabIdBySession((prev) => ({ ...prev, [sessionId]: taskId }));
      interactiveSubProcessRef.current.set(getSubProcessRouteKey(sessionId, agent), taskId);
      pendingDispatchRef.current.set(taskId, { taskId, dispatchId, sessionId });
      invoke<DispatcherSessionRuntimeState>("dispatcher_attach_checklist_subprocess", {
        workspaceId: sessionId,
        dispatchId,
        taskId,
      })
        .then((state) => {
          if (activeSessionIdRef.current === sessionId) {
            dispatcherChatRef.current?.applyRuntimeState(state);
          }
        })
        .catch(console.error);
      idleInjectedTaskIdsRef.current.delete(taskId);
      closedSubprocessTaskIdsRef.current.delete(taskId);
      onRetainTaskBuffers([taskId]);
    },
    [onRetainTaskBuffers, onStartSubProcess],
  );

  // Monitor task completion for subprocesses → inject result back
  useEffect(() => {
    const unsub = listen<{ task_id: string; status: string }>("task-status", (e) => {
      const { task_id, status } = e.payload;
      const targetSubProcess = allSubProcessesRef.current.find((sp) => sp.id === task_id);
      if (!targetSubProcess) return;
      const routeKey = getSubProcessRouteKey(targetSubProcess.sessionId, targetSubProcess.agent);

      if (status === "running" || status === "input_required") {
        closedSubprocessTaskIdsRef.current.delete(task_id);
        setDismissedSubProcessIds((prev) => {
          if (!prev.has(task_id)) return prev;
          const next = new Set(prev);
          next.delete(task_id);
          return next;
        });
        setActiveSubTabIdBySession((prev) => ({ ...prev, [targetSubProcess.sessionId]: task_id }));
        interactiveSubProcessRef.current.set(routeKey, task_id);
        invoke("dispatcher_mark_subprocess_running", { taskId: task_id }).catch(console.error);
        return;
      }

      if (status === "stopped") {
        pendingDispatchRef.current.delete(task_id);
        idleInjectedTaskIdsRef.current.delete(task_id);
        interactiveSubProcessRef.current.delete(routeKey);
        invoke("dispatcher_mark_subprocess_stopped", { taskId: task_id }).catch(console.error);
        return;
      }

      if (status !== "done" && status !== "failed" && status !== "cancelled") return;

      const alreadyInjectedByIdle = idleInjectedTaskIdsRef.current.has(task_id);
      closedSubprocessTaskIdsRef.current.add(task_id);
      invoke("dispatcher_mark_subprocess_finished", { taskId: task_id }).catch(console.error);

      const isDone = status === "done";

      // If this subprocess was voluntarily exited via /exit, skip result injection
      if (exitedSubprocessesRef.current.has(task_id)) {
        exitedSubprocessesRef.current.delete(task_id);
        pendingDispatchRef.current.delete(task_id);
        idleInjectedTaskIdsRef.current.delete(task_id);
        if (interactiveSubProcessRef.current.get(routeKey) === task_id) {
          interactiveSubProcessRef.current.delete(routeKey);
        }
        return;
      }

      const pending = pendingDispatchRef.current.get(task_id);
      if (!pending || alreadyInjectedByIdle) {
        pendingDispatchRef.current.delete(task_id);
        idleInjectedTaskIdsRef.current.delete(task_id);
        if (interactiveSubProcessRef.current.get(routeKey) === task_id) {
          interactiveSubProcessRef.current.delete(routeKey);
        }
        return;
      }

      pendingDispatchRef.current.delete(task_id);
      idleInjectedTaskIdsRef.current.delete(task_id);
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

      if (interactiveSubProcessRef.current.get(routeKey) === task_id) {
        interactiveSubProcessRef.current.delete(routeKey);
      }

      dispatcherChatRef.current?.continueWithResult(
        resultText,
        dispatchState,
        pending.sessionId,
        pending.dispatchId,
      );
    });
    return () => {
      unsub.then((fn) => fn()).catch(() => {});
    };
  }, [getTaskRestoreState]);

  // 监听 session watcher 发出的“当前轮次完成”信号，允许在同一子进程内继续注入。
  useEffect(() => {
    const unsub = listen<{ task_id: string; output: string }>("dispatcher-subprocess-idle", (e) => {
      const { task_id, output } = e.payload;
      if (closedSubprocessTaskIdsRef.current.has(task_id)) return;
      if (exitedSubprocessesRef.current.has(task_id)) return;
      if (idleInjectedTaskIdsRef.current.has(task_id)) return;
      const targetSubProcess = allSubProcessesRef.current.find((sp) => sp.id === task_id);
      if (!targetSubProcess) return;

      const cleaned = cleanTerminalOutput(output);
      const agent = targetSubProcess?.agent ?? "claude";
      const agentLabel = getSubProcessAgentLabel(agent);
      const resultText = `${agentLabel} 当前轮次已完成，子进程仍在运行，可继续注入后续指令。\n\n终端输出：\n${cleaned}`;
      const pending = pendingDispatchRef.current.get(task_id);
      const sessionId = pending?.sessionId ?? targetSubProcess.sessionId;
      if (!sessionId) return;
      pendingDispatchRef.current.delete(task_id);
      idleInjectedTaskIdsRef.current.add(task_id);
      interactiveSubProcessRef.current.set(getSubProcessRouteKey(sessionId, agent), task_id);
      invoke("dispatcher_mark_subprocess_round_completed", { taskId: task_id }).catch(
        console.error,
      );
      dispatcherChatRef.current?.continueWithResult(
        resultText,
        "round_completed",
        sessionId,
        pending?.dispatchId,
      );
    });
    return () => {
      unsub.then((fn) => fn()).catch(() => {});
    };
  }, []);  // allSubProcesses accessed via ref to avoid listener re-registration

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
      const agentLabel = getSubProcessAgentLabel(agent);
      if (!activeSp) {
        dispatcherChatRef.current?.continueWithResult(
          `${agentLabel} 子进程续写失败：未找到运行中的子进程。`,
          "process_failed",
          sessionId,
        );
        return;
      }

      interactiveSubProcessRef.current.set(routeKey, activeSp.id);
      idleInjectedTaskIdsRef.current.delete(activeSp.id);
      invoke("dispatcher_mark_subprocess_running", { taskId: activeSp.id }).catch((error) => {
        dispatcherChatRef.current?.continueWithResult(
          `${agentLabel} 子进程状态同步失败：${toErrorMessage(error)}`,
          "process_failed",
          sessionId,
        );
      });
      const submittedText = text.replace(/(?:\r?\n)+$/, "");
      invoke("dispatcher_send_to_subprocess", { taskId: activeSp.id, text: submittedText }).catch(
        (error) => {
          dispatcherChatRef.current?.continueWithResult(
            `${agentLabel} 子进程续写失败：${toErrorMessage(error)}`,
            "process_failed",
            sessionId,
          );
        },
      );
    },
    [subProcesses],
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
      const agentLabel = getSubProcessAgentLabel(agent);
      if (!activeSp) {
        dispatcherChatRef.current?.continueWithResult(
          `${agentLabel} 子进程退出失败：未找到运行中的子进程。`,
          "process_failed",
          sessionId,
        );
        return;
      }

      interactiveSubProcessRef.current.set(routeKey, activeSp.id);
      exitedSubprocessesRef.current.add(activeSp.id);
      closedSubprocessTaskIdsRef.current.add(activeSp.id);
      invoke("dispatcher_exit_subprocess", { taskId: activeSp.id }).catch((error) => {
        exitedSubprocessesRef.current.delete(activeSp.id);
        closedSubprocessTaskIdsRef.current.delete(activeSp.id);
        dispatcherChatRef.current?.continueWithResult(
          `${agentLabel} 子进程退出失败：${toErrorMessage(error)}`,
          "process_failed",
          sessionId,
        );
      });
    },
    [subProcesses],
  );

  const handleCloseSubTab = useCallback(
    (id: string) => {
      const targetSubProcess = subProcesses.find((sp) => sp.id === id);
      setDismissedSubProcessIds((prev) => {
        if (prev.has(id)) return prev;
        const next = new Set(prev);
        next.add(id);
        return next;
      });
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
      onReleaseTaskBuffers([id]);
      if (
        targetSubProcess?.status !== "running" &&
        targetSubProcess?.status !== "pending_approval" &&
        targetSubProcess?.status !== "stopped"
      ) {
        onRemoveTaskBuffers([id]);
      }
    },
    [onReleaseTaskBuffers, onRemoveTaskBuffers, subProcesses],
  );

  const handleStopSessionSubProcesses = useCallback(
    async (sessionId: string) => {
      const runningSubProcesses = allSubProcesses.filter(
        (subProcess) => subProcess.sessionId === sessionId && subProcess.status === "running",
      );
      await Promise.all(
        runningSubProcesses.map((subProcess) => {
          closedSubprocessTaskIdsRef.current.add(subProcess.id);
          return onStopTask(subProcess.id);
        }),
      );
    },
    [allSubProcesses, onStopTask],
  );

  const handleResumeSessionSubProcesses = useCallback(
    async (sessionId: string) => {
      const resumableTasks = tasks.filter(
        (task) =>
          task.projectId === project.id &&
          task.dispatcherSessionId === sessionId &&
          task.status === "stopped",
      );

      if (resumableTasks.length === 0) return;

      setDismissedSubProcessIds((prev) => {
        const next = new Set(prev);
        for (const task of resumableTasks) {
          next.delete(task.id);
        }
        return next;
      });
      setActiveSubTabIdBySession((prev) => ({
        ...prev,
        [sessionId]: resumableTasks[0].id,
      }));

      await Promise.all(
        resumableTasks.map((task) => {
          closedSubprocessTaskIdsRef.current.delete(task.id);
          idleInjectedTaskIdsRef.current.delete(task.id);
          onRetainTaskBuffers([task.id]);
          return onResumeTask(task);
        }),
      );
    },
    [onResumeTask, onRetainTaskBuffers, project.id, tasks],
  );

  const handleOpenPlanDocument = useCallback(
    (path: string) => {
      handleFileSelect(path, getPathBasename(path));
      showEditorWorkbench();
    },
    [handleFileSelect, showEditorWorkbench],
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
        sessionSidebarCollapsed={sessionSidebarCollapsed}
        onExpandSessionSidebar={() => setSessionSidebarCollapsed(false)}
        onSwitch={onSwitchProject}
        onOpen={onOpen}
      />
      {!sessionSidebarCollapsed && (
        <SessionPanel
          project={project}
          activeSessionId={activeSessionId}
          subprocessRunningSessionIds={subprocessRunningSessionIds}
          onSelectSession={handleSelectSession}
          onBack={onBack}
          onCollapse={() => setSessionSidebarCollapsed(true)}
          isDark={isDark}
          themeMode={themeMode}
          systemPrefersDark={systemPrefersDark}
          onThemeModeChange={onThemeModeChange}
          onToggleTheme={onToggleTheme}
        />
      )}
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
                  <Suspense fallback={<LazyPaneFallback label="会话加载中..." />}>
                    {activeSessionId ? (
                      <MarkdownLinkProvider onOpenUrl={handleOpenMarkdownLink}>
                        <DispatcherChat
                          ref={dispatcherChatRef}
                          sessionId={activeSessionId}
                          projectPath={project.path}
                          mcpStatus={mcpStatus}
                          mcpChecking={mcpChecking}
                          layoutMode={showEditorPane ? "split" : "single"}
                          subProcesses={allSubProcesses}
                          onDispatchApproved={handleDispatchApproved}
                          onDispatchRejected={handleDispatchRejected}
                          onDispatchContinue={handleDispatchContinue}
                          onDispatchExit={handleDispatchExit}
                          onStopActiveRun={handleStopSessionSubProcesses}
                          onResumeStoppedRun={handleResumeSessionSubProcesses}
                          onOpenMcpStatus={() => setShowMcpStatus(true)}
                          onOpenSettings={() => setShowDispatcherSettings(true)}
                          onOpenPlanDocument={handleOpenPlanDocument}
                          onClosePanel={() => setShowSessionWorkbench(false)}
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
                  borderLeft: workbenchColumnCount === 2 ? "1px solid var(--border-dim)" : "none",
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
                        isDark={isDark}
                      />
                    )}
                  </Suspense>
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
        {subProcesses.length > 0 && (
          <Suspense fallback={null}>
            <SubProcessTabs
              subProcesses={subProcesses}
              activeSessionId={activeSessionId}
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
            />
          </Suspense>
        )}
        {showShellTerminal && (
          <Suspense fallback={null}>
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
          </Suspense>
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
              <Suspense fallback={<LazyPaneFallback label="文件列表加载中..." />}>
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
        </div>
      )}

      <RightToolbar
        activePanel={rightPanel}
        onToggle={handleTogglePanel}
        terminalActive={showShellTerminal}
        onToggleTerminal={() => setShowShellTerminal((v) => !v)}
      />

      {showDispatcherSettings && (
        <Suspense fallback={null}>
          <AppSettingsDialog
            isDark={isDark}
            themeMode={themeMode}
            systemPrefersDark={systemPrefersDark}
            onThemeModeChange={onThemeModeChange}
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
    </div>
  );
}

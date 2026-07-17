import { lazy, Suspense, useState, useEffect, useMemo, useCallback, useRef } from "react";
import { open as openDialog, confirm } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Project, Task, TaskStatus, AgentType, PermissionMode } from "./types";
import { isActiveTaskStatus } from "./types";
import { WelcomePage } from "./components/WelcomePage";
import { useToast } from "./components/Toast";
import { useTerminalManager } from "./hooks/useTerminalManager";
import "./App.css";

const ProjectPage = lazy(() =>
  import("./components/ProjectPage").then((module) => ({ default: module.ProjectPage })),
);

function AppPaneFallback() {
  return (
    <div
      style={{
        height: "100%",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        color: "var(--text-muted)",
        fontSize: 13,
      }}
    >
      加载中...
    </div>
  );
}

function persistProjects(projects: Project[], onError: (msg: string) => void) {
  invoke("save_projects", { projects }).catch((e: unknown) => {
    console.error(e);
    onError(`保存项目列表失败：${String(e)}`);
  });
}

function App() {
  const { showToast } = useToast();

  const [projects, setProjects] = useState<Project[]>([]);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [activeProject, setActiveProject] = useState<Project | null>(null);
  const [mountedProjectIds, setMountedProjectIds] = useState<string[]>([]);

  const tm = useTerminalManager();

  // ── Debounced task persistence ─────────────────────────────────────────────
  const persistTimersRef = useRef<Record<string, number>>({});
  const tasksRef = useRef<Task[]>([]);
  tasksRef.current = tasks;

  const debouncedPersistProjectTasks = useCallback(
    (projectId: string, allTasks: Task[]) => {
      const timers = persistTimersRef.current;
      if (timers[projectId]) {
        window.clearTimeout(timers[projectId]);
      }
      timers[projectId] = window.setTimeout(() => {
        delete timers[projectId];
        invoke("save_project_tasks", {
          projectId,
          tasks: allTasks.filter((t) => t.projectId === projectId),
        }).catch((e: unknown) => {
          console.error(e);
          showToast(`保存任务失败（项目 ${projectId}）：${String(e)}`);
        });
      }, 400);
    },
    [showToast],
  );

  const updateTaskStatus = useCallback(
    (
      taskId: string,
      status: TaskStatus,
      extra?: Pick<Task, "attentionRequestedAt">,
      failureReason?: string,
    ) => {
      setTasks((prev) => {
        let changed = false;
        const next = prev.map((task) => {
          if (task.id !== taskId) return task;

          const attentionRequestedAt =
            status === "input_required" ? (extra?.attentionRequestedAt ?? Date.now()) : undefined;

          const nextFailureReason =
            status === "failed" ? (failureReason ?? task.failureReason) : undefined;

          if (
            task.status === status &&
            task.attentionRequestedAt === attentionRequestedAt &&
            task.failureReason === nextFailureReason
          ) {
            return task;
          }

          changed = true;
          const updated: Task = { ...task, status, attentionRequestedAt };
          if (nextFailureReason) updated.failureReason = nextFailureReason;
          if (!nextFailureReason) delete updated.failureReason;
          return updated;
        });

        if (changed) {
          const task = next.find((t) => t.id === taskId);
          if (task) debouncedPersistProjectTasks(task.projectId, next);
        }
        return changed ? next : prev;
      });
    },
    [debouncedPersistProjectTasks],
  );

  const updateTaskSession = useCallback(
    (taskId: string, sessionId: string, sessionPath: string) => {
      setTasks((prev) => {
        let changed = false;
        const next = prev.map((task) => {
          if (task.id !== taskId) return task;
          if (task.agent === "claude") {
            if (task.claudeSessionId === sessionId && task.claudeSessionPath === sessionPath) {
              return task;
            }
            changed = true;
            return { ...task, claudeSessionId: sessionId, claudeSessionPath: sessionPath };
          }

          if (task.codexSessionId === sessionId && task.codexSessionPath === sessionPath) {
            return task;
          }
          changed = true;
          return { ...task, codexSessionId: sessionId, codexSessionPath: sessionPath };
        });

        if (changed) {
          const task = next.find((t) => t.id === taskId);
          if (task) debouncedPersistProjectTasks(task.projectId, next);
        }
        return changed ? next : prev;
      });
    },
    [debouncedPersistProjectTasks],
  );

  const mountProject = useCallback((projectId: string) => {
    setMountedProjectIds((prev) => (prev.includes(projectId) ? prev : [...prev, projectId]));
  }, []);

  useEffect(() => {
    async function init() {
      // Load projects from ~/.jkcodingagent/projects.json
      const loadedProjects = await invoke<Project[]>("load_projects");
      setProjects(loadedProjects);

      // Load tasks for all known projects
      const chunks = await Promise.all(
        loadedProjects.map((p) => invoke<Task[]>("load_project_tasks", { projectId: p.id })),
      );
      setTasks(chunks.flat());
    }

    init().catch(console.error);
  }, []);

  // Tauri event listeners (agent-output is handled inside useTerminalManager)
  useEffect(() => {
    const p1 = listen<{ task_id: string; status: TaskStatus; failure_reason?: string }>(
      "task-status",
      (e) => {
        const { task_id, status, failure_reason } = e.payload;
        if (status === "failed" && failure_reason) {
          tm.writeErrorToTerminal(task_id, `\r\n错误：${failure_reason}\r\n`);
          showToast(`Agent 执行失败：${failure_reason}`);
        }
        updateTaskStatus(task_id, status, undefined, failure_reason);
        if (status === "done" || status === "failed" || status === "cancelled") {
          tm.removeInactiveTaskBuffers([task_id]);
        }
      },
    );
    const p2 = listen<{ task_id: string; session_id: string; session_path: string }>(
      "task-session",
      (e) => {
        const { task_id, session_id, session_path } = e.payload;
        updateTaskSession(task_id, session_id, session_path);
      },
    );
    return () => {
      p1.then((fn) => fn()).catch(() => {});
      p2.then((fn) => fn()).catch(() => {});
    };
  }, [tm, showToast, updateTaskSession, updateTaskStatus]);

  async function handleOpen() {
    const selected = await openDialog({ directory: true, multiple: false });
    if (!selected) return;
    const path = selected as string;
    const name = path.split("/").pop() || path;
    const project: Project = { id: crypto.randomUUID(), name, path, lastOpenedAt: Date.now() };
    setProjects((prev) => {
      const next = [project, ...prev.filter((p) => p.path !== path)];
      persistProjects(next, showToast);
      return next;
    });
    setActiveProject(project);
    mountProject(project.id);
    invoke("init_project_config", { projectPath: path }).catch((e: unknown) => {
      showToast(`初始化项目配置失败：${String(e)}`, "warning");
    });
  }

  function handleProjectClick(project: Project) {
    const updated = { ...project, lastOpenedAt: Date.now() };
    setProjects((prev) => {
      const next = prev.map((p) => (p.id === project.id ? updated : p));
      persistProjects(next, showToast);
      return next;
    });
    setActiveProject(updated);
    mountProject(updated.id);
    invoke("init_project_config", { projectPath: project.path }).catch((e: unknown) => {
      showToast(`初始化项目配置失败：${String(e)}`, "warning");
    });
  }

  function handleBack() {
    setActiveProject(null);
  }

  function invokeStartDispatcherSubprocess(task: Task, projectPath: string) {
    if (!task.dispatcherDispatchId || !task.dispatcherSessionId || !task.dispatcherDescription) {
      throw new Error("调度子进程缺少 Dispatcher 元数据");
    }

    invoke("start_dispatcher_subprocess", {
      taskId: task.id,
      projectPath,
      prompt: task.prompt,
      agent: task.agent,
      permissionMode: task.permissionMode,
      cols: tm.terminalSizeRef.current.cols,
      rows: tm.terminalSizeRef.current.rows,
      dispatcherDispatchId: task.dispatcherDispatchId,
      dispatcherSessionId: task.dispatcherSessionId,
      dispatcherDescription: task.dispatcherDescription,
    }).catch((err: unknown) => {
      const msg = err instanceof Error ? err.message : String(err);
      tm.writeErrorToTerminal(task.id, `\r\n错误：${msg}\r\n`);
      updateTaskStatus(task.id, "failed", undefined, msg);
    });
  }

  function invokeResumeDispatcherSubprocess(task: Task, projectPath: string) {
    const sessionId = task.agent === "claude" ? task.claudeSessionId : task.codexSessionId;
    if (!sessionId) {
      showToast("当前任务尚未记录可恢复的会话 ID，暂时无法继续。", "warning");
      return Promise.resolve();
    }

    if (!task.dispatcherDispatchId) {
      showToast("当前任务不是调度子进程，不能从这里继续。", "warning");
      return Promise.resolve();
    }

    if (!task.dispatcherSessionId || !task.dispatcherDescription) {
      showToast("当前调度子进程缺少 Dispatcher 元数据，暂时无法继续。", "warning");
      return Promise.resolve();
    }

    return invoke("resume_dispatcher_subprocess", {
      taskId: task.id,
      projectPath,
      agent: task.agent,
      sessionId,
      prompt: task.prompt,
      permissionMode: task.permissionMode,
      cols: tm.terminalSizeRef.current.cols,
      rows: tm.terminalSizeRef.current.rows,
      dispatcherDispatchId: task.dispatcherDispatchId,
      dispatcherSessionId: task.dispatcherSessionId,
      dispatcherDescription: task.dispatcherDescription,
    }).catch((err: unknown) => {
      const msg = err instanceof Error ? err.message : String(err);
      tm.writeErrorToTerminal(task.id, `\r\n错误：${msg}\r\n`);
      updateTaskStatus(task.id, "stopped");
      showToast(`继续任务失败：${msg}`);
    });
  }

  function handleStartDispatcherSubprocess(
    project: Project,
    {
      prompt,
      agent,
      permissionMode,
      dispatcherDispatchId,
      dispatcherSessionId,
      dispatcherDescription,
    }: {
      prompt: string;
      agent: AgentType;
      permissionMode: PermissionMode;
      dispatcherDispatchId: string;
      dispatcherSessionId: string;
      dispatcherDescription: string;
    },
  ): string {
    const task: Task = {
      id: crypto.randomUUID(),
      projectId: project.id,
      prompt,
      agent,
      permissionMode,
      status: "pending",
      createdAt: Date.now(),
      dispatcherDispatchId,
      dispatcherSessionId,
      dispatcherDescription,
    };
    setTasks((prev) => {
      const next = [task, ...prev];
      debouncedPersistProjectTasks(task.projectId, next);
      return next;
    });

    // Ensure the project is mounted so the task's terminal can be initialized in the background.
    mountProject(project.id);

    tm.resetTaskTerminal(task.id);
    invokeStartDispatcherSubprocess(task, project.path);
    return task.id;
  }

  async function deleteTasks(taskIds: string[]) {
    if (taskIds.length === 0) return;

    // Phase 1: stop active tasks on backend (read via ref, not setTasks)
    const toDelete = new Set(taskIds);
    const activeTasks = tasksRef.current.filter(
      (t) => toDelete.has(t.id) && isActiveTaskStatus(t.status),
    );
    await Promise.allSettled(
      activeTasks.map((task) =>
        invoke("stop_task", { taskId: task.id }).catch((e: unknown) => {
          showToast(`停止任务失败：${String(e)}`);
        }),
      ),
    );

    // Phase 2: atomically remove from state and persist
    setTasks((prev) => {
      const next = prev.filter((task) => !toDelete.has(task.id));
      if (next.length === prev.length) return prev;
      const affectedProjectIds = new Set(
        prev.filter((t) => toDelete.has(t.id)).map((t) => t.projectId),
      );
      affectedProjectIds.forEach((pid) => debouncedPersistProjectTasks(pid, next));
      return next;
    });

    tm.removeTaskBuffers(taskIds);
  }

  async function handleDeleteProject(projectId: string) {
    const project = projects.find((p) => p.id === projectId);
    if (!project) return;
    const ok = await confirm(`确定删除项目“${project.name}”及其全部任务记录吗？`, {
      title: "删除项目",
      kind: "warning",
    });
    if (!ok) return;
    const projectTaskIds = tasks.filter((t) => t.projectId === projectId).map((t) => t.id);
    await deleteTasks(projectTaskIds);
    setProjects((prev) => {
      const next = prev.filter((p) => p.id !== projectId);
      persistProjects(next, showToast);
      return next;
    });
    setMountedProjectIds((prev) => prev.filter((id) => id !== projectId));
    setActiveProject((prev) => (prev?.id === projectId ? null : prev));
  }

  const sortedProjects = useMemo(
    () => [...projects].sort((a, b) => b.lastOpenedAt - a.lastOpenedAt),
    [projects],
  );
  const railProjects = useMemo(
    () => [...projects].sort((a, b) => Number(a.id) - Number(b.id)),
    [projects],
  );
  const mountedProjects = useMemo(
    () =>
      mountedProjectIds
        .map((id) => projects.find((project) => project.id === id))
        .filter((project): project is Project => !!project),
    [mountedProjectIds, projects],
  );

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100%",
        background: "var(--bg-root)",
        overflow: "hidden",
        position: "relative",
      }}
    >
      <div
        style={{
          position: "absolute",
          inset: 0,
          overflow: "hidden",
        }}
      >
        {mountedProjects.map((project) => (
          <Suspense key={project.id} fallback={<AppPaneFallback />}>
            <ProjectPage
              project={project}
              visible={activeProject?.id === project.id}
              allProjects={railProjects}
              tasks={tasks}
              getTaskRestoreState={tm.getTaskRestoreState}
              onStartSubProcess={(taskInput) =>
                handleStartDispatcherSubprocess(project, taskInput)
              }
              onStopTask={(taskId) =>
                invoke("stop_task", { taskId })
                  .catch((e: unknown) => {
                    showToast(`停止任务失败：${String(e)}`);
                  })
                  .then(() => undefined)
              }
              onResumeTask={(task) => {
                const sessionId =
                  task.agent === "claude" ? task.claudeSessionId : task.codexSessionId;
                if (!sessionId) {
                  showToast("当前任务尚未记录可恢复的会话 ID，暂时无法继续。", "warning");
                  return Promise.resolve();
                }
                updateTaskStatus(task.id, "pending");
                return invokeResumeDispatcherSubprocess(task, project.path).then(() => undefined);
              }}
              onInput={tm.handleInput}
              onResize={tm.handleResize}
              onRegisterTerminal={tm.handleRegisterTerminal}
              onTerminalReady={tm.handleTerminalReady}
              onSnapshot={tm.handleSnapshot}
              onRetainTaskBuffers={tm.retainTaskBuffers}
              onReleaseTaskBuffers={tm.releaseTaskBuffers}
              onRemoveTaskBuffers={tm.removeTaskBuffers}
              onBack={handleBack}
              onSwitchProject={handleProjectClick}
              onOpen={handleOpen}
            />
          </Suspense>
        ))}
      </div>
      {!activeProject && (
        <div
          style={{
            position: "absolute",
            inset: 0,
            zIndex: 5,
          }}
        >
          <WelcomePage
            projects={sortedProjects}
            onOpen={handleOpen}
            onProjectClick={handleProjectClick}
            onDeleteProject={handleDeleteProject}
          />
        </div>
      )}
    </div>
  );
}

export default App;

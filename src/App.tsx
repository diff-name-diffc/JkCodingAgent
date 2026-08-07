import { lazy, Suspense, useState, useEffect, useLayoutEffect, useMemo, useCallback, useRef } from "react";
import { open as openDialog, confirm } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import type { Project, Task } from "./types";
import { isActiveTaskStatus } from "./types";
import { WelcomePage } from "./components/WelcomePage";
import { useToast } from "./components/Toast";
import { normalizeThemePreference, persistThemePreference } from "./lib/theme";
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

  // 主题的权威来源是后端 settings.json；main.tsx 的 initializeTheme() 只用
  // localStorage 缓存做首帧渲染，这里启动后立刻以后端值校准（缓存缺失或过期时
  // 会错误落回 system，导致设置了亮色却显示暗色）。
  useEffect(() => {
    invoke<{ theme?: string }>("load_app_settings")
      .then((settings) => {
        persistThemePreference(normalizeThemePreference(settings.theme));
      })
      .catch(() => {
        // 读取失败时保留 initializeTheme() 已应用的缓存主题。
      });
  }, []);

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

  function handleCloseProject(target: Project) {
    // 只更新挂载列表，激活项目的回退统一交给下方收敛守卫：这里若基于本次渲染
    // 快照计算 nextId，同批连续关闭多个项目的边界场景下可能把激活项目切到同样
    // 正被关闭的项目上，与守卫逻辑重复且结果不可靠。
    setMountedProjectIds((prev) => prev.filter((id) => id !== target.id));
  }

  // 收敛守卫（激活项目回退的唯一出口）：activeProject 指向已卸载项目时
  // （关闭激活项目、同批连续关闭等场景），回退到最近挂载的项目或欢迎页，
  // 避免出现「激活项目不在挂载列表」导致的空白视图。用 useLayoutEffect 在
  // 绘制前同步收敛，关闭激活项目时不会出现「全部项目页隐藏」的闪烁帧。
  useLayoutEffect(() => {
    if (activeProject && !mountedProjectIds.includes(activeProject.id)) {
      const nextId = mountedProjectIds[mountedProjectIds.length - 1] ?? null;
      setActiveProject(nextId ? (projects.find((p) => p.id === nextId) ?? null) : null);
    }
  }, [activeProject, mountedProjectIds, projects]);

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
      <div data-tauri-drag-region className="ai-titlebar" aria-hidden="true" />
      <div
        style={{
          position: "relative",
          flex: 1,
          minHeight: 0,
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
                openProjects={mountedProjects}
                tasks={tasks}
                onBack={handleBack}
                onSwitchProject={handleProjectClick}
                onCloseProject={handleCloseProject}
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
    </div>
  );
}

export default App;

import { useState, useEffect, useRef } from "react";
import { Plus, ChevronsRight, ChevronRight } from "lucide-react";
import type { Project, Task } from "../types";
import { ProjectAvatar } from "./ProjectAvatar";

type ProjectStatus = "attention" | "running" | null;

function getProjectStatus(tasks: Task[], projectId: string): ProjectStatus {
  const projectTasks = tasks.filter((t) => t.projectId === projectId);
  if (projectTasks.some((t) => t.status === "input_required")) return "attention";
  if (projectTasks.some((t) => t.status === "running" || t.status === "pending")) return "running";
  return null;
}

const STATUS_COLOR: Record<Exclude<ProjectStatus, null>, string> = {
  attention: "var(--color-warning, #f59e0b)",
  running: "var(--color-success, #22c55e)",
};

function StatusBadge({ status }: { status: ProjectStatus }) {
  if (!status) return null;
  return (
    <span
      className={`ai-project-status-dot ai-project-status-${status}`}
      style={{ background: STATUS_COLOR[status] }}
    />
  );
}

function RailItem({
  project,
  isActive,
  status,
  onSwitch,
}: {
  project: Project;
  isActive: boolean;
  status: ProjectStatus;
  onSwitch: (p: Project) => void;
}) {
  return (
    <button
      title={project.name}
      onClick={() => onSwitch(project)}
      className={isActive ? "ai-project-rail-item is-active" : "ai-project-rail-item"}
    >
      <ProjectAvatar name={project.name} size={28} />
      <StatusBadge status={status} />
    </button>
  );
}

function ProjectDrawer({
  projects,
  allTasks,
  activeProjectId,
  onSwitch,
  onClose,
}: {
  projects: Project[];
  allTasks: Task[];
  activeProjectId: string;
  onSwitch: (p: Project) => void;
  onClose: () => void;
}) {
  const drawerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (drawerRef.current && !drawerRef.current.contains(e.target as Node)) {
        onClose();
      }
    }
    document.addEventListener("mousedown", handleClick);
    return () => document.removeEventListener("mousedown", handleClick);
  }, [onClose]);

  return (
    <div ref={drawerRef} className="ai-project-drawer">
      <div className="ai-project-drawer-title">项目</div>
      <div className="ai-project-drawer-list">
        {projects.map((project) => {
          const status = getProjectStatus(allTasks, project.id);
          const isActive = project.id === activeProjectId;
          return (
            <button
              key={project.id}
              className={isActive ? "ai-project-drawer-row is-active" : "ai-project-drawer-row"}
              onClick={() => {
                onSwitch(project);
                onClose();
              }}
            >
              <div className="ai-project-drawer-avatar">
                <ProjectAvatar name={project.name} size={28} />
                {status && (
                  <span
                    className="ai-project-drawer-status-dot"
                    style={{ background: STATUS_COLOR[status] }}
                  />
                )}
              </div>
              <span className="ai-project-drawer-name">{project.name}</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}

export function ProjectRail({
  projects,
  allTasks,
  activeProjectId,
  sessionSidebarCollapsed,
  onExpandSessionSidebar,
  onSwitch,
  onOpen,
}: {
  projects: Project[];
  allTasks: Task[];
  activeProjectId: string;
  sessionSidebarCollapsed: boolean;
  onExpandSessionSidebar: () => void;
  onSwitch: (project: Project) => void;
  onOpen: () => void;
}) {
  const [drawerOpen, setDrawerOpen] = useState(false);

  return (
    <div className="ai-project-rail" style={{ zIndex: drawerOpen ? 50 : "auto" }}>
      {sessionSidebarCollapsed && (
        <button
          className="ai-project-rail-control is-attention"
          title="展开会话列表"
          onClick={onExpandSessionSidebar}
        >
          <ChevronRight size={15} strokeWidth={2.5} />
        </button>
      )}

      {projects.map((project) => (
        <RailItem
          key={project.id}
          project={project}
          isActive={project.id === activeProjectId}
          status={getProjectStatus(allTasks, project.id)}
          onSwitch={(p) => {
            onSwitch(p);
            setDrawerOpen(false);
          }}
        />
      ))}

      <div className="ai-project-rail-spacer" />

      <button
        className={drawerOpen ? "ai-project-rail-control is-active" : "ai-project-rail-control"}
        title="显示全部项目"
        onClick={() => setDrawerOpen((v) => !v)}
      >
        <ChevronsRight size={14} strokeWidth={2.5} className="ai-project-rail-control-icon" />
      </button>

      <button
        className="ai-project-rail-control ai-project-rail-add"
        title="打开项目"
        onClick={onOpen}
      >
        <Plus size={14} strokeWidth={2.5} />
      </button>

      {drawerOpen && (
        <ProjectDrawer
          projects={projects}
          allTasks={allTasks}
          activeProjectId={activeProjectId}
          onSwitch={onSwitch}
          onClose={() => setDrawerOpen(false)}
        />
      )}
    </div>
  );
}

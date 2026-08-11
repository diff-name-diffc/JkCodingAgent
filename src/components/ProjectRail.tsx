import { useState, useEffect, useRef } from "react";
import { Plus, ChevronsRight, ChevronRight, X } from "lucide-react";
import type { Project } from "../types";
import { ProjectAvatar } from "./ProjectAvatar";

function RailItem({
  project,
  isActive,
  onSwitch,
  onClose,
}: {
  project: Project;
  isActive: boolean;
  onSwitch: (p: Project) => void;
  onClose?: (p: Project) => void;
}) {
  // 外层为纯布局容器：项目切换与关闭是两个平级的真实 <button>，
  // 避免「按钮套按钮」的嵌套交互控件（屏幕阅读器无法正确暴露内层按钮，
  // 且会把关闭按钮的可访问名称拼进外层按钮名称）。
  return (
    <div className="ai-project-rail-item-slot">
      <button
        type="button"
        className={isActive ? "ai-project-rail-item is-active" : "ai-project-rail-item"}
        title={project.name}
        aria-label={project.name}
        aria-current={isActive ? "true" : undefined}
        onClick={() => onSwitch(project)}
      >
        <ProjectAvatar name={project.name} size={28} />
      </button>
      {onClose && (
        <button
          type="button"
          className="ai-project-rail-close"
          title={`关闭 ${project.name}`}
          aria-label={`关闭 ${project.name}`}
          onClick={() => onClose(project)}
        >
          <X size={10} strokeWidth={2.5} />
        </button>
      )}
    </div>
  );
}

function ProjectDrawer({
  projects,
  activeProjectId,
  onSwitch,
  onClose,
}: {
  projects: Project[];
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
  allProjects = projects,
  activeProjectId,
  sessionSidebarCollapsed,
  onExpandSessionSidebar,
  onSwitch,
  onCloseProject,
  onOpen,
}: {
  /** 已打开的项目窗口，展示为栏内图标。 */
  projects: Project[];
  /** 全部已知项目，展示在抽屉里供打开/重新打开。 */
  allProjects?: Project[];
  activeProjectId: string;
  sessionSidebarCollapsed: boolean;
  onExpandSessionSidebar: () => void;
  onSwitch: (project: Project) => void;
  onCloseProject?: (project: Project) => void;
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
          onSwitch={(p) => {
            onSwitch(p);
            setDrawerOpen(false);
          }}
          onClose={onCloseProject}
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
          projects={allProjects}
          activeProjectId={activeProjectId}
          onSwitch={onSwitch}
          onClose={() => setDrawerOpen(false)}
        />
      )}
    </div>
  );
}

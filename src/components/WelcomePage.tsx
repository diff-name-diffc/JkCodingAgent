import { lazy, Suspense, useState, useMemo } from "react";
import type React from "react";
import {
  Search,
  FolderOpen,
  GitBranch,
  Layers,
  Plus,
  Trash2,
  MessageCircle,
  Workflow,
} from "lucide-react";
import type { Project } from "../types";
import { shortenPath } from "../utils";
import { ProjectAvatar } from "./ProjectAvatar";
import { SidebarFooterActions } from "./SidebarFooterActions";
import {
  AiEmptyState,
  AiSectionHeader,
  AiStatusPill,
} from "./ui/sci-fi-shell";
import appLogo from "../assets/app-logo.png";

const HomeChatPage = lazy(() =>
  import("./HomeChatPage").then((module) => ({ default: module.HomeChatPage })),
);

const ArchitectureView = lazy(() =>
  import("./architecture/ArchitectureView").then((module) => ({
    default: module.ArchitectureView,
  })),
);

function WelcomePaneFallback() {
  return (
    <div className="ai-home-pane ai-empty-state">
      加载中...
    </div>
  );
}

function SidebarItem({
  icon,
  label,
  active,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  active?: boolean;
  onClick?: () => void;
}) {
  return (
    <button
      type="button"
      className={`ai-home-nav-item${active ? " is-active" : ""}`}
      onClick={onClick}
      aria-label={label}
      aria-current={active ? "page" : undefined}
    >
      <span className="ai-home-nav-icon">{icon}</span>
      <span className="ai-home-nav-label">{label}</span>
    </button>
  );
}

function WelcomeEmpty({ hasProjects, onOpen }: { hasProjects: boolean; onOpen: () => void }) {
  return (
    <AiEmptyState
      icon={<FolderOpen size={40} strokeWidth={1.2} />}
      title={hasProjects ? "没有匹配的项目" : "还没有项目"}
      description={!hasProjects ? "打开一个本地 Git 仓库以开始使用" : undefined}
      action={
        !hasProjects ? (
          <button className="ai-home-primary-btn" onClick={onOpen}>
            <FolderOpen size={14} strokeWidth={2} />
            打开项目文件夹...
          </button>
        ) : undefined
      }
    />
  );
}

export function WelcomePage({
  projects,
  onOpen,
  onProjectClick,
  onDeleteProject,
}: {
  projects: Project[];
  onOpen: () => void;
  onProjectClick: (p: Project) => void;
  onDeleteProject: (projectId: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [hov, setHov] = useState<string | null>(null);
  const [searchFocused, setSearchFocused] = useState(false);
  const [view, setView] = useState<"projects" | "chat" | "architecture">("chat");

  const filtered = useMemo(() => {
    if (!query.trim()) return projects;
    const q = query.toLowerCase();
    return projects.filter(
      (p) => p.name.toLowerCase().includes(q) || p.path.toLowerCase().includes(q),
    );
  }, [projects, query]);

  return (
    <div className="ai-home-shell">
      <div className="ai-home-layout">
        <div className="ai-home-nav">
          <div className="ai-home-brand" aria-label="JKCodingAgent">
            <div className="ai-home-brand-icon">
              <img
                src={appLogo}
                alt="JKCodingAgent"
              />
            </div>
            <span className="ai-home-brand-title">JKCodingAgent</span>
          </div>

          <nav className="ai-home-nav-list" aria-label="主导航">
            <SidebarItem
              icon={<MessageCircle size={18} />}
              label="聊天"
              active={view === "chat"}
              onClick={() => setView("chat")}
            />
            <SidebarItem
              icon={<Layers size={18} />}
              label="项目"
              active={view === "projects"}
              onClick={() => setView("projects")}
            />
            <SidebarItem
              icon={<Workflow size={18} />}
              label="架构设计"
              active={view === "architecture"}
              onClick={() => setView("architecture")}
            />
          </nav>

          <div className="ai-home-nav-footer">
            <SidebarFooterActions />
          </div>
        </div>

        {view === "chat" && (
          <Suspense fallback={<WelcomePaneFallback />}>
            <HomeChatPage />
          </Suspense>
        )}

        {view === "architecture" && (
          <Suspense fallback={<WelcomePaneFallback />}>
            <ArchitectureView />
          </Suspense>
        )}

        {view === "projects" && (
          <div className="ai-home-pane ai-home-projects">
            <div className="ai-home-search-row">
              <div className={`ai-field ai-home-search${searchFocused ? " is-focused" : ""}`}>
                <Search
                  size={15}
                  strokeWidth={1.9}
                  color="var(--text-muted)"
                  style={{ flexShrink: 0 }}
                />
                <input
                  placeholder="搜索项目"
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  onFocus={() => setSearchFocused(true)}
                  onBlur={() => setSearchFocused(false)}
                  autoFocus
                />
              </div>

              <div className="ai-home-search-actions">
                <button className="ai-home-primary-btn" onClick={onOpen}>
                  <Plus size={14} strokeWidth={2.3} />
                  <span>打开项目</span>
                </button>
              </div>
            </div>

            <AiSectionHeader
              title="项目"
              caption={
                query.trim() ? `找到 ${filtered.length} 个结果` : `共 ${projects.length} 个项目`
              }
            />

            <div className="ai-project-grid">
              {filtered.length === 0 ? (
                <WelcomeEmpty hasProjects={projects.length > 0} onOpen={onOpen} />
              ) : (
                filtered.map((p) => {
                  return (
                    <div
                      key={p.id}
                      role="button"
                      tabIndex={0}
                      className={`ai-list-row ai-project-card${hov === p.id ? " is-active" : ""}`}
                      onMouseEnter={() => setHov(p.id)}
                      onMouseLeave={() => setHov(null)}
                      onClick={() => onProjectClick(p)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" || e.key === " ") {
                          e.preventDefault();
                          onProjectClick(p);
                        }
                      }}
                    >
                      <ProjectAvatar name={p.name} size={34} />

                      <div className="ai-project-card-main">
                        <div className="ai-project-name">{p.name}</div>
                        <div className="ai-project-meta">{shortenPath(p.path)}</div>
                      </div>

                      {p.branch ? (
                        <AiStatusPill tone="accent">
                          <GitBranch size={10} strokeWidth={2} />
                          {p.branch}
                        </AiStatusPill>
                      ) : (
                        <AiStatusPill>本地</AiStatusPill>
                      )}

                      <button
                        className="ai-project-delete-btn"
                        onClick={(e) => {
                          e.stopPropagation();
                          onDeleteProject(p.id);
                        }}
                        title="删除项目"
                      >
                        <Trash2 size={14} strokeWidth={1.8} />
                      </button>
                    </div>
                  );
                })
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

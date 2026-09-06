import { lazy, Suspense, useState, useMemo } from "react";
import { Search, FolderOpen, Plus, Trash2 } from "lucide-react";
import type { Project } from "../types";
import { formatRelativeTime, shortenPath } from "../utils";
import { ProjectAvatar } from "./ProjectAvatar";
import { AiEmptyState, AiSectionHeader } from "./ui/sci-fi-shell";
import { AppRail } from "./shell/AppRail";

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
  view,
  onViewChange,
  onOpen,
  onProjectClick,
  onDeleteProject,
}: {
  projects: Project[];
  view: "projects" | "chat" | "architecture";
  onViewChange: (view: "projects" | "chat" | "architecture") => void;
  onOpen: () => void;
  onProjectClick: (p: Project) => void;
  onDeleteProject: (projectId: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [searchFocused, setSearchFocused] = useState(false);

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
        <AppRail space={view} onSpaceChange={onViewChange} />

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
              title="最近项目"
              caption={
                query.trim() ? `找到 ${filtered.length} 个结果` : `共 ${projects.length} 个项目`
              }
            />

            <div className="ai-project-recent-list">
              {filtered.length === 0 ? (
                <WelcomeEmpty hasProjects={projects.length > 0} onOpen={onOpen} />
              ) : (
                <ul role="list" className="ai-project-recent-ul">
                  {filtered.map((p) => (
                    <li key={p.id} className="ai-project-recent-item">
                      <button
                        type="button"
                        className="ai-project-recent-row"
                        onClick={() => onProjectClick(p)}
                        title={`${p.name} · ${p.path}`}
                      >
                        <ProjectAvatar name={p.name} size={28} />
                        <span className="ai-project-recent-main">
                          <span className="ai-project-name">{p.name}</span>
                          <span className="ai-project-meta">
                            {shortenPath(p.path)} ·{" "}
                            {formatRelativeTime(new Date(p.lastOpenedAt).toISOString())}
                          </span>
                        </span>
                      </button>
                      <button
                        type="button"
                        className="ai-project-delete-btn"
                        onClick={() => onDeleteProject(p.id)}
                        title="删除项目"
                        aria-label={`删除项目 ${p.name}`}
                      >
                        <Trash2 size={14} strokeWidth={1.8} />
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

import { lazy, Suspense, useState, useMemo } from "react";
import type React from "react";
import {
  Search,
  FolderOpen,
  GitBranch,
  Layers,
  Plus,
  Trash2,
  BarChart2,
  MessageCircle,
} from "lucide-react";
import type { Project, ThemeMode } from "../types";
import { shortenPath } from "../utils";
import { ProjectAvatar } from "./ProjectAvatar";
import { SidebarFooterActions } from "./SidebarFooterActions";
import {
  AiEmptyState,
  AiSectionHeader,
  AiStatusPill,
} from "./ui/sci-fi-shell";
import appLogo from "../assets/app-logo.png";
import s from "../styles";

const AnalyticsDashboard = lazy(() =>
  import("./AnalyticsDashboard").then((module) => ({ default: module.AnalyticsDashboard })),
);
const HomeChatPage = lazy(() =>
  import("./HomeChatPage").then((module) => ({ default: module.HomeChatPage })),
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
  meta,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  active?: boolean;
  meta?: string;
  onClick?: () => void;
}) {
  return (
    <div
      className={`ai-home-nav-item${active ? " is-active" : ""}`}
      onClick={onClick}
    >
      <span style={{ display: "flex", alignItems: "center" }}>{icon}</span>
      <span className="ai-home-nav-label">{label}</span>
      {meta && <span style={s.sidebarItemMeta}>{meta}</span>}
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
  onOpen,
  onProjectClick,
  onDeleteProject,
  isDark,
  themeMode,
  systemPrefersDark,
  onThemeModeChange,
  onToggleTheme,
}: {
  projects: Project[];
  onOpen: () => void;
  onProjectClick: (p: Project) => void;
  onDeleteProject: (projectId: string) => void;
  isDark: boolean;
  themeMode: ThemeMode;
  systemPrefersDark: boolean;
  onThemeModeChange: (mode: ThemeMode) => void;
  onToggleTheme: () => void;
}) {
  const [query, setQuery] = useState("");
  const [hov, setHov] = useState<string | null>(null);
  const [searchFocused, setSearchFocused] = useState(false);
  const [view, setView] = useState<"projects" | "chat" | "analytics">("chat");

  const filtered = useMemo(() => {
    if (!query.trim()) return projects;
    const q = query.toLowerCase();
    return projects.filter(
      (p) => p.name.toLowerCase().includes(q) || p.path.toLowerCase().includes(q),
    );
  }, [projects, query]);

  return (
    <div className="ai-home-shell ai-migrated-home" style={s.welcomeBody}>
      <div className="ai-home-layout">
        <div className="ai-home-nav" style={s.sidebar}>
          <div style={s.sidebarBrand}>
            <div className="ai-home-brand-icon" style={s.sidebarBrandIcon}>
              <img
                src={appLogo}
                alt="JKCodingAgent"
                style={{ width: "100%", height: "100%", borderRadius: 9, objectFit: "cover" }}
              />
            </div>
            <div>
              <div style={s.sidebarBrandTitle}>JKCodingAgent</div>
              <div style={s.sidebarBrandMeta}>智能体工作区</div>
            </div>
          </div>

          <nav style={s.sidebarNav}>
            <div style={s.sidebarSectionTitle}>工作区</div>
            <SidebarItem
              icon={<MessageCircle size={15} />}
              label="聊天"
              active={view === "chat"}
              onClick={() => setView("chat")}
            />
            <SidebarItem
              icon={<Layers size={15} />}
              label="项目"
              active={view === "projects"}
              onClick={() => setView("projects")}
            />
            <SidebarItem
              icon={<BarChart2 size={15} />}
              label="分析"
              active={view === "analytics"}
              onClick={() => setView("analytics")}
            />
          </nav>

          <div style={s.sidebarFooter}>
            <SidebarFooterActions
              isDark={isDark}
              themeMode={themeMode}
              systemPrefersDark={systemPrefersDark}
              onThemeModeChange={onThemeModeChange}
              onToggleTheme={onToggleTheme}
            />
          </div>
        </div>

        {view === "chat" ? (
          <Suspense fallback={<WelcomePaneFallback />}>
            <HomeChatPage
              isDark={isDark}
              themeMode={themeMode}
              systemPrefersDark={systemPrefersDark}
              onThemeModeChange={onThemeModeChange}
            />
          </Suspense>
        ) : view === "analytics" ? (
          <Suspense fallback={<WelcomePaneFallback />}>
            <AnalyticsDashboard projects={projects} />
          </Suspense>
        ) : (
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

              <div style={s.actionRow}>
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

                      <div style={{ flex: 1, minWidth: 0 }}>
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

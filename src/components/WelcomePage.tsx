import { useState, useMemo } from "react";
import {
  Search,
  FolderOpen,
  GitBranch,
  Layers,
  Plus,
  Trash2,
  BarChart2,
  BookOpen,
  MessageCircle,
} from "lucide-react";
import type { Project, ThemeMode } from "../types";
import { getAvatarGradient, shortenPath } from "../utils";
import { ProjectAvatar } from "./ProjectAvatar";
import { SidebarFooterActions } from "./SidebarFooterActions";
import { AnalyticsDashboard } from "./AnalyticsDashboard";
import { KnowledgePage } from "./knowledge/KnowledgePage";
import { HomeChatPage } from "./HomeChatPage";
import appLogo from "../assets/app-logo.png";
import s from "../styles";

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
      style={{
        ...s.sidebarItem,
        background: active ? "var(--bg-selected)" : "transparent",
        color: active ? "var(--text-primary)" : "var(--text-muted)",
      }}
      onClick={onClick}
    >
      <span style={{ display: "flex", alignItems: "center" }}>{icon}</span>
      <span style={{ marginLeft: 10, fontSize: 13, fontWeight: active ? 600 : 500 }}>{label}</span>
      {meta && <span style={s.sidebarItemMeta}>{meta}</span>}
    </div>
  );
}

function WelcomeEmpty({ hasProjects, onOpen }: { hasProjects: boolean; onOpen: () => void }) {
  return (
    <div style={s.emptyState}>
      <div style={{ marginBottom: 14, opacity: 0.4 }}>
        <FolderOpen size={40} strokeWidth={1.2} color="var(--text-hint)" />
      </div>
      <div
        style={{ fontSize: 14, fontWeight: 600, color: "var(--text-secondary)", marginBottom: 6 }}
      >
        {hasProjects ? "没有匹配的项目" : "还没有项目"}
      </div>
      {!hasProjects && (
        <>
          <div style={{ fontSize: 12.5, color: "var(--text-muted)", marginBottom: 20 }}>
            打开一个本地 Git 仓库以开始使用
          </div>
          <button style={s.emptyOpenBtn} onClick={onOpen}>
            <FolderOpen size={14} strokeWidth={2} />
            打开项目文件夹...
          </button>
        </>
      )}
    </div>
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
  const [view, setView] = useState<"projects" | "chat" | "analytics" | "knowledge">("chat");

  const filtered = useMemo(() => {
    if (!query.trim()) return projects;
    const q = query.toLowerCase();
    return projects.filter(
      (p) => p.name.toLowerCase().includes(q) || p.path.toLowerCase().includes(q),
    );
  }, [projects, query]);

  return (
    <div style={s.welcomeBody}>
      <div style={s.welcomeMain}>
        <div style={s.sidebar}>
          <div style={s.sidebarBrand}>
            <div style={s.sidebarBrandIcon}>
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
            <SidebarItem
              icon={<BookOpen size={15} />}
              label="知识库"
              active={view === "knowledge"}
              onClick={() => setView("knowledge")}
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
          <HomeChatPage
            isDark={isDark}
            themeMode={themeMode}
            systemPrefersDark={systemPrefersDark}
            onThemeModeChange={onThemeModeChange}
          />
        ) : view === "analytics" ? (
          <AnalyticsDashboard projects={projects} />
        ) : view === "knowledge" ? (
          <KnowledgePage />
        ) : (
          <div style={s.welcomePane}>
            <div style={s.searchRow}>
              <div
                style={{
                  ...s.searchBox,
                  borderColor: searchFocused ? "var(--border-focus)" : "var(--border-medium)",
                  boxShadow: searchFocused ? "0 0 0 3px var(--accent-subtle)" : "none",
                }}
              >
                <Search
                  size={15}
                  strokeWidth={1.9}
                  color="var(--text-muted)"
                  style={{ flexShrink: 0 }}
                />
                <input
                  style={s.searchInput}
                  placeholder="搜索项目"
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  onFocus={() => setSearchFocused(true)}
                  onBlur={() => setSearchFocused(false)}
                  autoFocus
                />
              </div>

              <div style={s.actionRow}>
                <button style={s.primaryActionBtn} onClick={onOpen}>
                  <Plus size={14} strokeWidth={2.3} />
                  <span>打开项目</span>
                </button>
              </div>
            </div>

            <div style={s.projectSectionHeader}>
              <div>
                <div style={s.projectSectionTitle}>项目</div>
                <div style={s.projectSectionCaption}>
                  {query.trim() ? `找到 ${filtered.length} 个结果` : `共 ${projects.length} 个项目`}
                </div>
              </div>
            </div>

            <div style={s.projectList}>
              {filtered.length === 0 ? (
                <WelcomeEmpty hasProjects={projects.length > 0} onOpen={onOpen} />
              ) : (
                filtered.map((p) => {
                  const [from] = getAvatarGradient(p.name);
                  return (
                    <div
                      key={p.id}
                      role="button"
                      tabIndex={0}
                      style={{
                        ...s.projectItem,
                        background: hov === p.id ? "var(--bg-hover)" : "transparent",
                        borderColor: hov === p.id ? "var(--border-medium)" : "transparent",
                      }}
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
                      <ProjectAvatar
                        name={p.name}
                        size={34}
                        style={{ boxShadow: hov === p.id ? `0 10px 18px ${from}26` : "none" }}
                      />

                      <div style={{ flex: 1, minWidth: 0 }}>
                        <div style={s.projectName}>{p.name}</div>
                        <div style={s.projectMeta}>{shortenPath(p.path)}</div>
                      </div>

                      {p.branch ? (
                        <span style={s.branchBadge}>
                          <GitBranch size={10} strokeWidth={2} />
                          {p.branch}
                        </span>
                      ) : (
                        <span style={s.projectTag}>本地</span>
                      )}

                      <button
                        style={{
                          marginLeft: 8,
                          padding: "4px 6px",
                          background: "transparent",
                          border: "none",
                          borderRadius: 6,
                          cursor: "pointer",
                          color: "var(--text-muted)",
                          display: "flex",
                          alignItems: "center",
                          opacity: hov === p.id ? 1 : 0,
                          transition: "opacity 0.15s, color 0.15s",
                        }}
                        onMouseEnter={(e) => {
                          (e.currentTarget as HTMLButtonElement).style.color =
                            "var(--danger, #f87171)";
                        }}
                        onMouseLeave={(e) => {
                          (e.currentTarget as HTMLButtonElement).style.color = "var(--text-muted)";
                        }}
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

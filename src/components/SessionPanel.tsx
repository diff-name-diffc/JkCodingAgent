import { useState, useEffect, useMemo, useCallback, useRef } from "react";
import { Search, ChevronLeft, PanelLeftClose, Plus, Trash2, LoaderCircle } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { confirm } from "@tauri-apps/plugin-dialog";
import {
  cleanupDispatcherSession,
  withDispatcherSessionRunning,
  withDispatcherSessionsRunning,
} from "./dispatcherSessionStore";
import type { Project, ThemeMode, ProjectSession, SessionPage, SessionSearchResult } from "../types";
import { ProjectAvatar } from "./ProjectAvatar";
import { SidebarFooterActions } from "./SidebarFooterActions";
import { BranchBar } from "./task-panel/BranchBar";
import { useDispatcherSessionRunningSet } from "../hooks/useDispatcherSessionRunningSet";
import s from "../styles";

const PROJECT_PAGE_SIZE = 30;

function formatTime(timestampStr: string) {
  try {
    const d = new Date(timestampStr);
    return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  } catch {
    return timestampStr;
  }
}

function sortSessionsByUpdatedAt(sessions: ProjectSession[]) {
  return [...sessions].sort((left, right) => {
    const leftTime = Date.parse(left.updatedAt);
    const rightTime = Date.parse(right.updatedAt);
    return (Number.isNaN(rightTime) ? 0 : rightTime) - (Number.isNaN(leftTime) ? 0 : leftTime);
  });
}

export function SessionPanel({
  project,
  activeSessionId,
  onSelectSession,
  onBack,
  onCollapse,
  isDark,
  themeMode,
  systemPrefersDark,
  onThemeModeChange,
  onToggleTheme,
  subprocessRunningSessionIds = new Set<string>(),
}: {
  project: Project;
  activeSessionId: string | null;
  onSelectSession: (id: string | null) => void;
  subprocessRunningSessionIds?: Set<string>;
  onBack: () => void;
  onCollapse: () => void;
  isDark: boolean;
  themeMode: ThemeMode;
  systemPrefersDark: boolean;
  onThemeModeChange: (mode: ThemeMode) => void;
  onToggleTheme: () => void;
}) {
  const [query, setQuery] = useState("");
  const [searchResults, setSearchResults] = useState<SessionSearchResult[] | null>(null);
  const [sessions, setSessions] = useState<ProjectSession[]>([]);
  const [hasMore, setHasMore] = useState(false);
  const [, setTotal] = useState(0);
  const [loadingMore, setLoadingMore] = useState(false);
  const creatingSessionRef = useRef(false);
  const activeSessionIdRef = useRef(activeSessionId);
  const sentinelRef = useRef<HTMLDivElement | null>(null);
  const searchTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  activeSessionIdRef.current = activeSessionId;

  useEffect(() => {
    const unlisten = listen<ProjectSession>("dispatcher-session-updated", (event) => {
      const updatedSession = withDispatcherSessionRunning(event.payload);
      if (!updatedSession.projectId || updatedSession.projectId !== project.id) return;

      setSessions((prev) => {
        const exists = prev.some((session) => session.id === updatedSession.id);
        const next = exists
          ? prev.map((session) => (session.id === updatedSession.id ? updatedSession : session))
          : [updatedSession, ...prev];
        return sortSessionsByUpdatedAt(next);
      });
    });

    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, [project.id]);

  const handleNewSession = useCallback(async () => {
    if (creatingSessionRef.current) return;
    creatingSessionRef.current = true;
    try {
      const newSession = withDispatcherSessionRunning(await invoke<ProjectSession>("project_create_session", {
        projectId: project.id,
        title: "新会话",
      }));
      setSessions((prev) =>
        prev.some((session) => session.id === newSession.id) ? prev : [newSession, ...prev],
      );
      onSelectSession(newSession.id);
    } catch (err) {
      console.error("创建会话失败:", err);
    } finally {
      creatingSessionRef.current = false;
    }
  }, [onSelectSession, project.id]);

  const loadMore = useCallback(async () => {
    if (!hasMore || loadingMore) return;
    setLoadingMore(true);
    try {
      const page = await invoke<SessionPage<ProjectSession>>("project_list_sessions", {
        projectId: project.id,
        offset: sessions.length,
        pageSize: PROJECT_PAGE_SIZE,
      });
      setSessions((prev) => {
        const existing = new Set(prev.map((s) => s.id));
        const newItems = withDispatcherSessionsRunning(page.items.filter((s) => !existing.has(s.id)));
        return sortSessionsByUpdatedAt([...prev, ...newItems]);
      });
      setTotal(page.total);
      setHasMore(page.hasMore);
    } finally {
      setLoadingMore(false);
    }
  }, [hasMore, loadingMore, project.id, sessions.length]);

  useEffect(() => {
    let cancelled = false;

    async function load() {
      try {
        const page = await invoke<SessionPage<ProjectSession>>("project_list_sessions", {
          projectId: project.id,
          offset: 0,
          pageSize: PROJECT_PAGE_SIZE,
        });
        if (cancelled) return;
        const items = withDispatcherSessionsRunning(page.items);
        setSessions(items);
        setTotal(page.total);
        setHasMore(page.hasMore);
        const currentSessionId = activeSessionIdRef.current;
        if (items.length > 0) {
          if (!currentSessionId || !items.some((session) => session.id === currentSessionId)) {
            onSelectSession(items[0].id);
          }
        } else {
          await handleNewSession();
        }
      } catch (err) {
        console.error("加载会话失败:", err);
      }
    }

    load();

    return () => {
      cancelled = true;
    };
  }, [handleNewSession, onSelectSession, project.id]);

  useEffect(() => {
    if (!sentinelRef.current) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0].isIntersecting && hasMore && !loadingMore) {
          loadMore();
        }
      },
      { rootMargin: "200px" },
    );
    observer.observe(sentinelRef.current);
    return () => observer.disconnect();
  }, [hasMore, loadingMore, loadMore]);

  const sessionIds = useMemo(() => sessions.map((session) => session.id), [sessions]);
  const dispatcherRunningSessionIds = useDispatcherSessionRunningSet(sessionIds);

  useEffect(() => {
    if (searchTimerRef.current) clearTimeout(searchTimerRef.current);
    const trimmed = query.trim();
    if (!trimmed) {
      setSearchResults(null);
      return;
    }
    searchTimerRef.current = setTimeout(() => {
      invoke<SessionSearchResult[]>("session_search_keywords", {
        query: trimmed,
        limit: 20,
        kind: "project",
        projectId: project.id,
      })
        .then(setSearchResults)
        .catch(() => setSearchResults([]));
    }, 260);
    return () => {
      if (searchTimerRef.current) clearTimeout(searchTimerRef.current);
    };
  }, [query, project.id]);

  async function handleDeleteSession(id: string) {
    const ok = await confirm("确定永久删除这个会话吗？", {
      title: "删除会话",
      kind: "warning",
    });
    if (!ok) return;

    try {
      await invoke("project_delete_session", { sessionId: id });
      cleanupDispatcherSession(id);
      const remaining = sessions.filter((session) => session.id !== id);
      setSessions(remaining);
      setTotal((prev) => Math.max(0, prev - 1));
      if (activeSessionId === id) {
        onSelectSession(remaining[0]?.id ?? null);
      }
      if (remaining.length === 0) {
        await handleNewSession();
      }
    } catch (err) {
      console.error("删除会话失败:", err);
    }
  }

  const filtered = useMemo(() => {
    if (!query.trim()) return sessions;
    const q = query.toLowerCase();
    return sessions.filter((session) => session.title.toLowerCase().includes(q));
  }, [sessions, query]);

  return (
    <div className="ai-project-session-panel" style={s.taskPanel}>
      {/* Project header */}
      <div className="ai-project-session-header" style={s.panelHeader}>
        <button className="ai-project-session-icon-btn" style={s.backBtn} onClick={onBack} title="返回项目页">
          <ChevronLeft size={15} strokeWidth={2} />
        </button>
        <ProjectAvatar name={project.name} size={22} />
        <span className="ai-project-session-title" style={s.panelProjectName}>{project.name}</span>
        <button className="ai-project-session-icon-btn" style={s.backBtn} onClick={onCollapse} title="折叠会话列表">
          <PanelLeftClose size={15} strokeWidth={2} />
        </button>
      </div>

      {/* Branch bar */}
      <BranchBar projectPath={project.path} />

      {/* Search */}
      <div className="ai-field ai-project-session-search" style={s.panelSearchWrap}>
        <Search size={13} strokeWidth={2} color="var(--text-muted)" style={{ flexShrink: 0 }} />
        <input
          style={s.panelSearchInput}
          placeholder="搜索会话..."
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
      </div>

      {/* New Session row */}
      <div className="ai-project-session-actions" style={s.taskActionsRow}>
        <button
          className="ai-project-new-session-btn"
          style={{
            display: "flex",
            alignItems: "center",
            gap: 6,
            background: "none",
            border: "none",
            color: "var(--accent)",
            fontSize: 13,
            fontWeight: 500,
            cursor: "pointer",
            padding: "4px 8px",
            borderRadius: 6,
          }}
          onClick={handleNewSession}
        >
          <Plus size={14} strokeWidth={2.5} />
          新建会话
        </button>
      </div>

      <div className="ai-project-session-divider" style={s.taskDivider} />

      {/* Session list */}
      <div className="ai-project-session-list chat-scroll" style={s.taskListScroll}>
        {searchResults !== null ? (
          searchResults.length === 0 ? (
	            <div className="ai-project-session-empty" style={s.taskListEmpty}>没有找到匹配的会话</div>
          ) : (
            searchResults.map((r) => {
              const isRunning =
                dispatcherRunningSessionIds.has(r.sessionId) ||
                subprocessRunningSessionIds.has(r.sessionId);
              return (
	                <div
	                  key={r.sessionId}
                    className={activeSessionId === r.sessionId ? "ai-project-session-row is-active" : "ai-project-session-row"}
	                  onClick={() => onSelectSession(r.sessionId)}
                  style={{
                    ...s.taskCard,
                    background: activeSessionId === r.sessionId ? "var(--bg-selected)" : "transparent",
                  }}
                >
                  <div style={{ flex: 1, minWidth: 0 }}>
	                    <div className="ai-project-session-row-title" style={s.taskCardTitle}>{r.sessionTitle}</div>
                    {r.matchedKeywords.length > 0 && (
                      <div style={s.matchedKeywordsRow}>
                        {r.matchedKeywords.slice(0, 4).map((kw) => (
	                          <span key={kw} className="ai-project-keyword-tag" style={s.matchedKeywordTag}>
                            {kw}
                          </span>
                        ))}
                        {r.matchedKeywords.length > 4 && (
	                          <span className="ai-project-keyword-tag" style={{ ...s.matchedKeywordTag, opacity: 0.6 }}>
                            +{r.matchedKeywords.length - 4}
                          </span>
                        )}
                      </div>
                    )}
	                    <div className="ai-project-session-row-sub" style={s.taskCardSub}>{formatTime(r.updatedAt)}</div>
                  </div>
                  <div style={s.taskCardActions}>
                    {isRunning && (
                      <LoaderCircle size={13} className="spin" style={s.sessionRunningIcon} />
                    )}
	                    <button
                        className="ai-project-session-delete"
	                      style={s.taskDeleteBtn}
                      onClick={(e) => {
                        e.stopPropagation();
                        handleDeleteSession(r.sessionId);
                      }}
                      title="删除会话"
                    >
                      <Trash2 size={13} color="var(--text-muted)" />
                    </button>
                  </div>
                </div>
              );
            })
          )
        ) : (
          <>
	            {filtered.length === 0 && <div className="ai-project-session-empty" style={s.taskListEmpty}>没有找到会话</div>}
            {filtered.map((session) => {
              const isRunning =
                Boolean(session.isRunning) ||
                dispatcherRunningSessionIds.has(session.id) ||
                subprocessRunningSessionIds.has(session.id);
              return (
	                <div
	                  key={session.id}
                    className={activeSessionId === session.id ? "ai-project-session-row is-active" : "ai-project-session-row"}
	                  onClick={() => onSelectSession(session.id)}
                  style={{
                    ...s.taskCard,
                    background: activeSessionId === session.id ? "var(--bg-selected)" : "transparent",
                  }}
                >
                  <div style={{ flex: 1, minWidth: 0 }}>
	                    <div className="ai-project-session-row-title" style={s.taskCardTitle}>{session.title}</div>
	                    <div className="ai-project-session-row-sub" style={s.taskCardSub}>{formatTime(session.updatedAt)}</div>
                  </div>
                  <div style={s.taskCardActions}>
                    {isRunning && (
                      <LoaderCircle size={13} className="spin" style={s.sessionRunningIcon} />
                    )}
	                    <button
                        className="ai-project-session-delete"
	                      style={s.taskDeleteBtn}
                      onClick={(e) => {
                        e.stopPropagation();
                        handleDeleteSession(session.id);
                      }}
                      title="删除会话"
                    >
                      <Trash2 size={13} color="var(--text-muted)" />
                    </button>
                  </div>
                </div>
              );
            })}
            {hasMore && (
              <div ref={sentinelRef} style={{ height: 1, width: "100%" }} />
            )}
          </>
        )}
      </div>

      <div className="ai-project-session-footer" style={s.taskPanelFooter}>
        <SidebarFooterActions
          isDark={isDark}
          themeMode={themeMode}
          systemPrefersDark={systemPrefersDark}
          onThemeModeChange={onThemeModeChange}
          onToggleTheme={onToggleTheme}
          projectId={project.id}
          projectPath={project.path}
        />
      </div>
    </div>
  );
}

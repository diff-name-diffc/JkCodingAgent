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
import type {
  Project,
  ProjectSession,
  SessionKeyword,
  SessionPage,
  SessionSearchResult,
} from "../types";
import { ProjectAvatar } from "./ProjectAvatar";
import { SidebarFooterActions } from "./SidebarFooterActions";
import { BranchBar } from "./task-panel/BranchBar";
import { useDispatcherSessionRunningSet } from "../hooks/useDispatcherSessionRunningSet";

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
}: {
  project: Project;
  activeSessionId: string | null;
  onSelectSession: (id: string | null) => void;
  onBack: () => void;
  onCollapse: () => void;
}) {
  const [query, setQuery] = useState("");
  const [searchResults, setSearchResults] = useState<SessionSearchResult[] | null>(null);
  const [searching, setSearching] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [keywordsRevision, setKeywordsRevision] = useState(0);
  const [sessions, setSessions] = useState<ProjectSession[]>([]);
  const [hasMore, setHasMore] = useState(false);
  const [, setTotal] = useState(0);
  const [loadingMore, setLoadingMore] = useState(false);
  const creatingSessionRef = useRef(false);
  const activeSessionIdRef = useRef(activeSessionId);
  const sentinelRef = useRef<HTMLDivElement | null>(null);
  const searchRequestRef = useRef(0);
  activeSessionIdRef.current = activeSessionId;

  useEffect(() => {
    const unlistenSession = listen<ProjectSession>("dispatcher-session-updated", (event) => {
      const updatedSession = withDispatcherSessionRunning(event.payload);
      if (!updatedSession.projectId || updatedSession.projectId !== project.id) return;

      setSessions((prev) => {
        const existingSession = prev.find((session) => session.id === updatedSession.id);
        const normalizedSession = {
          ...updatedSession,
          keywords: updatedSession.keywords ?? existingSession?.keywords ?? [],
        };
        const next = existingSession
          ? prev.map((session) =>
              session.id === normalizedSession.id ? normalizedSession : session,
            )
          : [normalizedSession, ...prev];
        return sortSessionsByUpdatedAt(next);
      });
    });
    const unlistenKeywords = listen<{
      workspaceId: string;
      keywords: SessionKeyword[];
    }>("session-keywords-updated", (event) => {
      const { workspaceId, keywords } = event.payload;
      const values = keywords.map((keyword) => keyword.keyword);
      setSessions((prev) =>
        prev.map((session) =>
          session.id === workspaceId ? { ...session, keywords: values } : session,
        ),
      );
      setKeywordsRevision((revision) => revision + 1);
    });

    return () => {
      unlistenSession.then((fn) => fn()).catch(() => {});
      unlistenKeywords.then((fn) => fn()).catch(() => {});
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
    const trimmed = query.trim();
    const requestId = ++searchRequestRef.current;
    if (!trimmed) {
      setSearchResults(null);
      setSearching(false);
      setSearchError(null);
      return;
    }
    setSearchResults(null);
    setSearching(true);
    setSearchError(null);
    const timer = window.setTimeout(() => {
      invoke<SessionSearchResult[]>("session_search_keywords", {
        query: trimmed,
        limit: 20,
        kind: "project",
        projectId: project.id,
      })
        .then((results) => {
          if (searchRequestRef.current !== requestId) return;
          setSearchResults(results);
        })
        .catch((error: unknown) => {
          if (searchRequestRef.current !== requestId) return;
          console.error("搜索会话失败:", error);
          setSearchError(String(error));
        })
        .finally(() => {
          if (searchRequestRef.current === requestId) setSearching(false);
        });
    }, 260);
    return () => window.clearTimeout(timer);
  }, [keywordsRevision, project.id, query]);

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
      setSearchResults((results) =>
        results?.filter((result) => result.sessionId !== id) ?? null,
      );
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

  const hasSearchQuery = query.trim().length > 0;

  return (
    <div className="ai-project-session-panel ai-migrated-project">
      {/* Project header */}
      <div className="ai-project-session-header">
        <button className="ai-project-session-icon-btn" onClick={onBack} title="返回项目页">
          <ChevronLeft size={15} strokeWidth={2} />
        </button>
        <ProjectAvatar name={project.name} size={22} />
        <span className="ai-project-session-title">{project.name}</span>
        <button className="ai-project-session-icon-btn" onClick={onCollapse} title="折叠会话列表">
          <PanelLeftClose size={15} strokeWidth={2} />
        </button>
      </div>

      {/* New Session row (primary action) */}
      <div className="ai-project-session-actions">
        <button className="ai-project-new-session-btn" onClick={handleNewSession}>
          <Plus size={14} strokeWidth={2.5} />
          新建会话
        </button>
      </div>

      {/* Search */}
      <div className="ai-field ai-project-session-search">
        {searching ? (
          <LoaderCircle size={13} className="spin" color="var(--text-muted)" />
        ) : (
          <Search size={13} strokeWidth={2} color="var(--text-muted)" />
        )}
        <input
          placeholder="搜索会话..."
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
      </div>

      {/* Branch bar */}
      <BranchBar projectPath={project.path} />

      <div className="ai-project-session-divider" />

      {/* Session list */}
      <div className="ai-project-session-list chat-scroll">
        {hasSearchQuery ? (
          searching ? (
            <div className="ai-project-session-empty">正在搜索…</div>
          ) : searchError ? (
            <div className="ai-project-session-empty is-error" title={searchError}>
              搜索失败，请重试
            </div>
          ) : searchResults?.length === 0 ? (
            <div className="ai-project-session-empty">没有找到匹配的会话</div>
          ) : (
            searchResults?.map((r) => {
              const isRunning =
                dispatcherRunningSessionIds.has(r.sessionId);
              return (
                <div
                  key={r.sessionId}
                  className={activeSessionId === r.sessionId ? "ai-project-session-row is-active" : "ai-project-session-row"}
                  onClick={() => onSelectSession(r.sessionId)}
                >
                  <div className="ai-project-session-row-main">
                    <div className="ai-project-session-row-title">
                      <span>{r.sessionTitle}</span>
                      {isRunning && (
                        <LoaderCircle size={13} className="spin ai-project-session-running" />
                      )}
                    </div>
                    <div className="ai-project-session-row-sub">{formatTime(r.updatedAt)}</div>
                  </div>
                  <div className="ai-project-session-actions-inline">
                    <button
                      className="ai-project-session-delete"
                      onClick={(e) => {
                        e.stopPropagation();
                        handleDeleteSession(r.sessionId);
                      }}
                      title="删除会话"
                      aria-label="删除会话"
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
            {sessions.length === 0 && <div className="ai-project-session-empty">没有找到会话</div>}
            {sessions.map((session) => {
              const isRunning =
                Boolean(session.isRunning) ||
                dispatcherRunningSessionIds.has(session.id);
              return (
                <div
                  key={session.id}
                  className={activeSessionId === session.id ? "ai-project-session-row is-active" : "ai-project-session-row"}
                  onClick={() => onSelectSession(session.id)}
                >
                  <div className="ai-project-session-row-main">
                    <div className="ai-project-session-row-title">
                      <span>{session.title}</span>
                      {isRunning && (
                        <LoaderCircle size={13} className="spin ai-project-session-running" />
                      )}
                    </div>
                    <div className="ai-project-session-row-sub">{formatTime(session.updatedAt)}</div>
                  </div>
                  <div className="ai-project-session-actions-inline">
                    <button
                      className="ai-project-session-delete"
                      onClick={(e) => {
                        e.stopPropagation();
                        handleDeleteSession(session.id);
                      }}
                      title="删除会话"
                      aria-label="删除会话"
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

      <div className="ai-project-session-footer">
        <SidebarFooterActions
          projectId={project.id}
          projectPath={project.path}
        />
      </div>
    </div>
  );
}

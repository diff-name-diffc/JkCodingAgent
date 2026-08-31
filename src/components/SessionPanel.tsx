import { useState, useEffect, useMemo, useCallback, useRef, memo } from "react";
import { Search, ChevronLeft, PanelLeftClose, Plus, Trash2, LoaderCircle } from "lucide-react";
import { confirm } from "@tauri-apps/plugin-dialog";
import { cleanupDispatcherSession } from "./dispatcherSessionStore";
import type { Project } from "../types";
import { ProjectAvatar } from "./ProjectAvatar";
import { SidebarFooterActions } from "./SidebarFooterActions";
import { BranchBar } from "./task-panel/BranchBar";
import { useDispatcherSessionRunningSet } from "../hooks/useDispatcherSessionRunningSet";
import {
  flattenSessionPages,
  useCreateProjectSession,
  useDeleteProjectSession,
  useProjectSessionsQuery,
  useSessionListEventMerge,
  useSessionSearchQuery,
} from "../hooks/use-session-queries";

function formatTime(timestampStr: string) {
  try {
    const d = new Date(timestampStr);
    return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  } catch {
    return timestampStr;
  }
}

/**
 * 会话行：memo 隔离，避免高频会话更新事件（dispatcher-session-updated）
 * 触发整表重渲染；依赖 props 均为稳定回调与标量。
 */
const SessionRow = memo(function SessionRow({
  id,
  title,
  updatedAt,
  isActive,
  isRunning,
  onSelect,
  onDelete,
}: {
  id: string;
  title: string;
  updatedAt: string;
  isActive: boolean;
  isRunning: boolean;
  onSelect: (id: string) => void;
  onDelete: (id: string) => void;
}) {
  return (
    <div
      className={isActive ? "ai-project-session-row is-active" : "ai-project-session-row"}
      onClick={() => onSelect(id)}
    >
      <div className="ai-project-session-row-main">
        <div className="ai-project-session-row-title">
          <span>{title}</span>
          {isRunning && (
            <LoaderCircle size={13} className="spin ai-project-session-running" />
          )}
        </div>
        <div className="ai-project-session-row-sub">{formatTime(updatedAt)}</div>
      </div>
      <div className="ai-project-session-actions-inline">
        <button
          className="ai-project-session-delete"
          onClick={(e) => {
            e.stopPropagation();
            onDelete(id);
          }}
          title="删除会话"
          aria-label="删除会话"
        >
          <Trash2 size={13} color="var(--text-muted)" />
        </button>
      </div>
    </div>
  );
});

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
  const [debouncedQuery, setDebouncedQuery] = useState("");
  const creatingSessionRef = useRef(false);
  const activeSessionIdRef = useRef(activeSessionId);
  const sentinelRef = useRef<HTMLDivElement | null>(null);
  const initialSyncProjectRef = useRef<string | null>(null);
  activeSessionIdRef.current = activeSessionId;

  // 数据层：与聊天侧共用的会话列表 / 事件合并 / 搜索 hook（project scope）
  useSessionListEventMerge({ kind: "project", projectId: project.id });
  const sessionsQuery = useProjectSessionsQuery(project.id);
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = sessionsQuery;
  const sessions = useMemo(() => flattenSessionPages(sessionsQuery.data), [sessionsQuery.data]);
  const { mutateAsync: createProjectSession } = useCreateProjectSession();
  const { mutateAsync: deleteProjectSession } = useDeleteProjectSession();
  const searchQuery = useSessionSearchQuery({
    query: debouncedQuery,
    kind: "project",
    projectId: project.id,
  });

  // 搜索防抖（260ms，与聊天侧一致）；搜索结果缓存由共享数据层失效逻辑驱动刷新
  useEffect(() => {
    const timer = window.setTimeout(() => setDebouncedQuery(query), 260);
    return () => window.clearTimeout(timer);
  }, [query]);

  const handleNewSession = useCallback(async (): Promise<boolean> => {
    if (creatingSessionRef.current) return false;
    creatingSessionRef.current = true;
    try {
      const newSession = await createProjectSession({
        projectId: project.id,
        title: "新会话",
      });
      onSelectSession(newSession.id);
      return true;
    } catch (err) {
      console.error("创建会话失败:", err);
      return false;
    } finally {
      creatingSessionRef.current = false;
    }
  }, [createProjectSession, onSelectSession, project.id]);

  // 首屏同步：列表加载完成后校正选中项；空列表自动新建（每个项目仅执行一次）
  useEffect(() => {
    if (!sessionsQuery.isSuccess) return;
    if (initialSyncProjectRef.current === project.id) return;
    initialSyncProjectRef.current = project.id;
    const currentSessionId = activeSessionIdRef.current;
    if (sessions.length > 0) {
      if (!currentSessionId || !sessions.some((session) => session.id === currentSessionId)) {
        onSelectSession(sessions[0].id);
      }
    } else {
      // 创建失败时复位标记，允许列表下次变化时重试，
      // 避免面板永久停留在「没有找到会话」空态。
      void handleNewSession().then((ok) => {
        if (!ok) initialSyncProjectRef.current = null;
      });
    }
  }, [sessionsQuery.isSuccess, sessions, project.id, onSelectSession, handleNewSession]);

  useEffect(() => {
    if (!sentinelRef.current) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0].isIntersecting && hasNextPage && !isFetchingNextPage) {
          void fetchNextPage();
        }
      },
      { rootMargin: "200px" },
    );
    observer.observe(sentinelRef.current);
    return () => observer.disconnect();
  }, [hasNextPage, isFetchingNextPage, fetchNextPage]);

  const sessionIds = useMemo(() => sessions.map((session) => session.id), [sessions]);
  const dispatcherRunningSessionIds = useDispatcherSessionRunningSet(sessionIds);

  // 删除回调用 ref 读取最新列表/选中态，保持引用稳定供 memo 行跳过重渲染。
  const sessionsRef = useRef(sessions);
  sessionsRef.current = sessions;
  const handleDeleteSession = useCallback(
    async (id: string) => {
      const ok = await confirm("确定永久删除这个会话吗？", {
        title: "删除会话",
        kind: "warning",
      });
      if (!ok) return;

      try {
        await deleteProjectSession({ sessionId: id, projectId: project.id });
        cleanupDispatcherSession(id);
        const remaining = sessionsRef.current.filter((session) => session.id !== id);
        if (activeSessionIdRef.current === id) {
          onSelectSession(remaining[0]?.id ?? null);
        }
        if (remaining.length === 0) {
          await handleNewSession();
        }
      } catch (err) {
        console.error("删除会话失败:", err);
      }
    },
    [deleteProjectSession, project.id, onSelectSession, handleNewSession],
  );

  const handleSelect = useCallback(
    (id: string) => onSelectSession(id),
    [onSelectSession],
  );

  const trimmedQuery = query.trim();
  const hasSearchQuery = trimmedQuery.length > 0;
  const searchPending = hasSearchQuery && trimmedQuery !== debouncedQuery.trim();
  // 有结果可展示时不切到「正在搜索」：防抖窗口与后台重查期间保留旧结果
  // （useSessionSearchQuery 的 placeholderData），消除逐键闪烁。
  const searchResults = hasSearchQuery ? (searchQuery.data ?? null) : null;
  const searching = hasSearchQuery && !searchResults && (searchPending || searchQuery.isFetching);
  const searchError =
    hasSearchQuery && !searchPending && !searchResults && searchQuery.error
      ? String(searchQuery.error)
      : null;

  return (
    <div className="ai-project-session-panel">
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
            searchResults?.map((r) => (
              <SessionRow
                key={r.sessionId}
                id={r.sessionId}
                title={r.sessionTitle}
                updatedAt={r.updatedAt}
                isActive={activeSessionId === r.sessionId}
                isRunning={dispatcherRunningSessionIds.has(r.sessionId)}
                onSelect={handleSelect}
                onDelete={handleDeleteSession}
              />
            ))
          )
        ) : (
          <>
            {sessions.length === 0 && <div className="ai-project-session-empty">没有找到会话</div>}
            {sessions.map((session) => (
              <SessionRow
                key={session.id}
                id={session.id}
                title={session.title}
                updatedAt={session.updatedAt}
                isActive={activeSessionId === session.id}
                isRunning={
                  Boolean(session.isRunning) || dispatcherRunningSessionIds.has(session.id)
                }
                onSelect={handleSelect}
                onDelete={handleDeleteSession}
              />
            ))}
            {hasNextPage && (
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

import { useState, useEffect, useMemo, useCallback, useRef } from "react";
import { Search, ChevronLeft, PanelLeftClose, Plus, Trash2, LoaderCircle } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { confirm } from "@tauri-apps/plugin-dialog";
import { cleanupDispatcherSession } from "./dispatcherSessionStore";
import type { Project, ThemeMode, DispatcherSession } from "../types";
import { ProjectAvatar } from "./ProjectAvatar";
import { SidebarFooterActions } from "./SidebarFooterActions";
import { BranchBar } from "./task-panel/BranchBar";
import { useDispatcherSessionRunningSet } from "../hooks/useDispatcherSessionRunningSet";
import s from "../styles";

function formatTime(timestampStr: string) {
  try {
    const d = new Date(timestampStr);
    return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  } catch {
    return timestampStr;
  }
}

function sortSessionsByUpdatedAt(sessions: DispatcherSession[]) {
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
  const [sessions, setSessions] = useState<DispatcherSession[]>([]);
  const creatingSessionRef = useRef(false);
  const activeSessionIdRef = useRef(activeSessionId);
  activeSessionIdRef.current = activeSessionId;

  useEffect(() => {
    const unlisten = listen<DispatcherSession>("dispatcher-session-updated", (event) => {
      const updatedSession = event.payload;
      if (updatedSession.projectId !== project.id) return;

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
      const newSession = await invoke<DispatcherSession>("dispatcher_create_session", {
        projectId: project.id,
        title: "新会话",
      });
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

  // Load sessions on mount or when project changes
  useEffect(() => {
    let cancelled = false;

    async function load() {
      try {
        const loaded = await invoke<DispatcherSession[]>("dispatcher_list_sessions", {
          projectId: project.id,
        });
        if (cancelled) return;
        setSessions(loaded);
        const currentSessionId = activeSessionIdRef.current;
        if (loaded.length > 0) {
          if (!currentSessionId || !loaded.some((session) => session.id === currentSessionId)) {
            onSelectSession(loaded[0].id);
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

  const sessionIds = useMemo(() => sessions.map((session) => session.id), [sessions]);
  const dispatcherRunningSessionIds = useDispatcherSessionRunningSet(sessionIds);

  async function handleDeleteSession(id: string) {
    const ok = await confirm("确定永久删除这个会话吗？", {
      title: "删除会话",
      kind: "warning",
    });
    if (!ok) return;

    try {
      await invoke("dispatcher_delete_session", { sessionId: id });
      cleanupDispatcherSession(id);
      const remaining = sessions.filter((session) => session.id !== id);
      setSessions(remaining);
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
    <div style={s.taskPanel}>
      {/* Project header */}
      <div style={s.panelHeader}>
        <button style={s.backBtn} onClick={onBack} title="返回项目页">
          <ChevronLeft size={15} strokeWidth={2} />
        </button>
        <ProjectAvatar name={project.name} size={22} />
        <span style={s.panelProjectName}>{project.name}</span>
        <button style={s.backBtn} onClick={onCollapse} title="折叠会话列表">
          <PanelLeftClose size={15} strokeWidth={2} />
        </button>
      </div>

      {/* Branch bar */}
      <BranchBar projectPath={project.path} />

      {/* Search */}
      <div style={s.panelSearchWrap}>
        <Search size={13} strokeWidth={2} color="var(--text-muted)" style={{ flexShrink: 0 }} />
        <input
          style={s.panelSearchInput}
          placeholder="搜索会话..."
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
      </div>

      {/* New Session row */}
      <div style={s.taskActionsRow}>
        <button
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

      <div style={s.taskDivider} />

      {/* Session list */}
      <div style={s.taskListScroll}>
        {filtered.length === 0 && <div style={s.taskListEmpty}>没有找到会话</div>}
        {filtered.map((session) => {
          const isRunning =
            dispatcherRunningSessionIds.has(session.id) ||
            subprocessRunningSessionIds.has(session.id);
          return (
            <div
              key={session.id}
              onClick={() => onSelectSession(session.id)}
              style={{
                ...s.taskCard,
                background: activeSessionId === session.id ? "var(--bg-selected)" : "transparent",
              }}
            >
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={s.taskCardTitle}>{session.title}</div>
                <div style={s.taskCardSub}>{formatTime(session.updatedAt)}</div>
              </div>
              <div style={s.taskCardActions}>
                {isRunning && (
                  <LoaderCircle size={13} className="spin" style={s.sessionRunningIcon} />
                )}
                <button
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
      </div>

      <div style={s.taskPanelFooter}>
        <SidebarFooterActions
          isDark={isDark}
          themeMode={themeMode}
          systemPrefersDark={systemPrefersDark}
          onThemeModeChange={onThemeModeChange}
          onToggleTheme={onToggleTheme}
        />
      </div>
    </div>
  );
}

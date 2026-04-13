import { useState, useEffect, useMemo, useCallback, useRef } from "react";
import { Search, ChevronLeft, Plus, Trash2 } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import type { Project, ThemeMode, DispatcherSession } from "../types";
import { ProjectAvatar } from "./ProjectAvatar";
import { SidebarFooterActions } from "./SidebarFooterActions";
import { BranchBar } from "./task-panel/BranchBar";
import s from "../styles";

function formatTime(timestampStr: string) {
  try {
    const d = new Date(timestampStr);
    return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  } catch {
    return timestampStr;
  }
}

export function SessionPanel({
  project,
  activeSessionId,
  onSelectSession,
  onBack,
  isDark,
  themeMode,
  systemPrefersDark,
  onThemeModeChange,
  onToggleTheme,
}: {
  project: Project;
  activeSessionId: string | null;
  onSelectSession: (id: string | null) => void;
  onBack: () => void;
  isDark: boolean;
  themeMode: ThemeMode;
  systemPrefersDark: boolean;
  onThemeModeChange: (mode: ThemeMode) => void;
  onToggleTheme: () => void;
}) {
  const [query, setQuery] = useState("");
  const [sessions, setSessions] = useState<DispatcherSession[]>([]);
  const creatingSessionRef = useRef(false);

  const handleNewSession = useCallback(async () => {
    if (creatingSessionRef.current) return;
    creatingSessionRef.current = true;
    try {
      const newSession = await invoke<DispatcherSession>("dispatcher_create_session", {
        projectId: project.id,
        title: "New Session",
      });
      setSessions((prev) =>
        prev.some((session) => session.id === newSession.id) ? prev : [newSession, ...prev],
      );
      onSelectSession(newSession.id);
    } catch (err) {
      console.error("Failed to create session:", err);
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
        if (loaded.length > 0) {
          if (!activeSessionId || !loaded.some((session) => session.id === activeSessionId)) {
            onSelectSession(loaded[0].id);
          }
        } else {
          await handleNewSession();
        }
      } catch (err) {
        console.error("Failed to load sessions:", err);
      }
    }

    load();

    return () => {
      cancelled = true;
    };
  }, [activeSessionId, handleNewSession, onSelectSession, project.id]);

  async function handleDeleteSession(id: string) {
    const ok = await confirm(`Delete this session permanently?`, {
      title: "Delete Session",
      kind: "warning",
    });
    if (!ok) return;

    try {
      await invoke("dispatcher_delete_session", { sessionId: id });
      const remaining = sessions.filter((session) => session.id !== id);
      setSessions(remaining);
      if (activeSessionId === id) {
        onSelectSession(remaining[0]?.id ?? null);
      }
      if (remaining.length === 0) {
        await handleNewSession();
      }
    } catch (err) {
      console.error("Failed to delete session:", err);
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
        <button style={s.backBtn} onClick={onBack} title="Switch project">
          <ChevronLeft size={15} strokeWidth={2} />
        </button>
        <ProjectAvatar name={project.name} size={22} />
        <span style={s.panelProjectName}>{project.name}</span>
      </div>

      {/* Branch bar */}
      <BranchBar projectPath={project.path} />

      {/* Search */}
      <div style={s.panelSearchWrap}>
        <Search size={13} strokeWidth={2} color="var(--text-muted)" style={{ flexShrink: 0 }} />
        <input
          style={s.panelSearchInput}
          placeholder="Search sessions..."
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
          New Session
        </button>
      </div>

      <div style={s.taskDivider} />

      {/* Session list */}
      <div style={s.taskListScroll}>
        {filtered.length === 0 && <div style={s.taskListEmpty}>No sessions found</div>}
        {filtered.map((session) => (
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
            <button
              style={s.taskDeleteBtn}
              onClick={(e) => {
                e.stopPropagation();
                handleDeleteSession(session.id);
              }}
              title="Delete session"
            >
              <Trash2 size={13} color="var(--text-muted)" />
            </button>
          </div>
        ))}
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

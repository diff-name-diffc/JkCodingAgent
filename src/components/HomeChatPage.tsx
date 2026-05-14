import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { confirm } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { LoaderCircle, MessageCircle, MonitorDot, Plus, Search, Trash2 } from "lucide-react";
import type { BrowserStatus, DispatcherSession, ThemeMode } from "../types";
import { DispatcherChat } from "./DispatcherChat";
import { AppSettingsDialog } from "./AppSettingsDialog";
import { BrowserPanel } from "./BrowserPanel";
import { useDockedBrowserPanel } from "../hooks/useDockedBrowserPanel";
import { useDispatcherSessionRunningSet } from "../hooks/useDispatcherSessionRunningSet";
import s from "../styles";

const CHAT_SCOPE_ID = "__global_chat__";

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

export function HomeChatPage({
  isDark,
  themeMode,
  systemPrefersDark,
  onThemeModeChange,
}: {
  isDark: boolean;
  themeMode: ThemeMode;
  systemPrefersDark: boolean;
  onThemeModeChange: (mode: ThemeMode) => void;
}) {
  const [query, setQuery] = useState("");
  const [sessions, setSessions] = useState<DispatcherSession[]>([]);
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [showBrowserPanel, setShowBrowserPanel] = useState(false);
  const browserPanel = useDockedBrowserPanel("nezha.chat.browserPanelWidth");
  const creatingSessionRef = useRef(false);
  const activeSessionIdRef = useRef(activeSessionId);
  activeSessionIdRef.current = activeSessionId;

  const handleNewSession = useCallback(async () => {
    if (creatingSessionRef.current) return;
    creatingSessionRef.current = true;
    try {
      const newSession = await invoke<DispatcherSession>("dispatcher_create_session", {
        projectId: CHAT_SCOPE_ID,
        kind: "chat",
        title: "新聊天",
      });
      setSessions((prev) =>
        prev.some((session) => session.id === newSession.id) ? prev : [newSession, ...prev],
      );
      setActiveSessionId(newSession.id);
    } catch (error) {
      console.error("创建聊天失败:", error);
    } finally {
      creatingSessionRef.current = false;
    }
  }, []);

  const sessionIds = useMemo(() => sessions.map((session) => session.id), [sessions]);
  const runningSessionIds = useDispatcherSessionRunningSet(sessionIds);

  useEffect(() => {
    const unlisten = listen<DispatcherSession>("dispatcher-session-updated", (event) => {
      const updatedSession = event.payload;
      if (updatedSession.projectId !== CHAT_SCOPE_ID || updatedSession.kind !== "chat") return;

      setSessions((prev) => {
        const exists = prev.some((session) => session.id === updatedSession.id);
        const next = exists
          ? prev.map((session) => (session.id === updatedSession.id ? updatedSession : session))
          : [updatedSession, ...prev];
        return sortSessionsByUpdatedAt(next);
      });
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    const unlisten = listen<BrowserStatus>("browser-status", (event) => {
      if (
        event.payload.sessionId === activeSessionIdRef.current &&
        event.payload.state !== "closed"
      ) {
        setShowBrowserPanel(true);
      }
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    let cancelled = false;

    async function load() {
      try {
        const loaded = await invoke<DispatcherSession[]>("dispatcher_list_sessions", {
          projectId: CHAT_SCOPE_ID,
          kind: "chat",
        });
        if (cancelled) return;
        setSessions(loaded);
        const current = activeSessionIdRef.current;
        if (loaded.length > 0) {
          if (!current || !loaded.some((session) => session.id === current)) {
            setActiveSessionId(loaded[0].id);
          }
        } else {
          await handleNewSession();
        }
      } catch (error) {
        console.error("加载聊天失败:", error);
      }
    }

    load();

    return () => {
      cancelled = true;
    };
  }, [handleNewSession]);

  async function handleDeleteSession(id: string) {
    const ok = await confirm("确定永久删除这个聊天吗？", {
      title: "删除聊天",
      kind: "warning",
    });
    if (!ok) return;

    try {
      await invoke("dispatcher_delete_session", { sessionId: id });
      const remaining = sessions.filter((session) => session.id !== id);
      setSessions(remaining);
      if (activeSessionId === id) {
        setActiveSessionId(remaining[0]?.id ?? null);
      }
      if (remaining.length === 0) {
        await handleNewSession();
      }
    } catch (error) {
      console.error("删除聊天失败:", error);
    }
  }

  const filtered = useMemo(() => {
    if (!query.trim()) return sessions;
    const q = query.toLowerCase();
    return sessions.filter((session) => session.title.toLowerCase().includes(q));
  }, [sessions, query]);

  return (
    <div style={s.chatHomeBody}>
      <div style={s.chatSessionPanel}>
        <div style={s.panelHeader}>
          <div style={s.chatPanelIcon}>
            <MessageCircle size={15} />
          </div>
          <span style={s.panelProjectName}>聊天</span>
        </div>

        <div style={s.panelSearchWrap}>
          <Search size={13} strokeWidth={2} color="var(--text-muted)" style={{ flexShrink: 0 }} />
          <input
            style={s.panelSearchInput}
            placeholder="搜索聊天..."
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </div>

        <div style={s.taskActionsRow}>
          <button style={s.chatNewSessionBtn} onClick={handleNewSession}>
            <Plus size={14} strokeWidth={2.5} />
            新建聊天
          </button>
          <button
            style={s.chatNewSessionBtn}
            onClick={() => setShowBrowserPanel((value) => !value)}
            title="CloakBrowser"
          >
            <MonitorDot size={14} strokeWidth={2.5} />
            浏览器
          </button>
        </div>

        <div style={s.taskDivider} />

        <div style={s.taskListScroll}>
          {filtered.length === 0 && <div style={s.taskListEmpty}>没有找到聊天</div>}
          {filtered.map((session) => (
            <div
              key={session.id}
              onClick={() => setActiveSessionId(session.id)}
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
                {runningSessionIds.has(session.id) && (
                  <LoaderCircle size={13} className="spin" style={s.sessionRunningIcon} />
                )}
                <button
                  style={s.taskDeleteBtn}
                  onClick={(event) => {
                    event.stopPropagation();
                    handleDeleteSession(session.id);
                  }}
                  title="删除聊天"
                >
                  <Trash2 size={13} color="var(--text-muted)" />
                </button>
              </div>
            </div>
          ))}
        </div>
      </div>

      <div style={s.chatMainPane}>
        {activeSessionId ? (
          <DispatcherChat
            conversationKind="chat"
            sessionId={activeSessionId}
            layoutMode="single"
            onOpenSettings={() => setShowSettings(true)}
          />
        ) : (
          <div style={s.chatEmptyPane}>正在创建聊天...</div>
        )}
      </div>

      {showBrowserPanel && (
        <div style={{ position: "relative", display: "flex", flexShrink: 0 }}>
          <div
            onMouseDown={browserPanel.handleResizeStart}
            style={{
              position: "absolute",
              left: 0,
              top: 0,
              bottom: 0,
              width: 5,
              cursor: "col-resize",
              zIndex: 10,
            }}
          />
          <BrowserPanel
            sessionId={activeSessionId}
            width={browserPanel.effectiveWidth}
            active={showBrowserPanel}
            expanded={browserPanel.expanded}
            onToggleExpanded={browserPanel.toggleExpanded}
            onClose={() => setShowBrowserPanel(false)}
          />
        </div>
      )}

      {showSettings && (
        <AppSettingsDialog
          isDark={isDark}
          themeMode={themeMode}
          systemPrefersDark={systemPrefersDark}
          onThemeModeChange={onThemeModeChange}
          initialTab="aha"
          onClose={() => setShowSettings(false)}
        />
      )}
    </div>
  );
}

import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { BrowserStatus, ThemeMode } from "../types";
import { useDockedBrowserPanel } from "../hooks/useDockedBrowserPanel";
import { ChatSessionSidebar } from "./ChatSessionSidebar";
import s from "../styles";

const DispatcherChat = lazy(() =>
  import("./DispatcherChat").then((module) => ({ default: module.DispatcherChat })),
);
const AppSettingsDialog = lazy(() =>
  import("./AppSettingsDialog").then((module) => ({ default: module.AppSettingsDialog })),
);
const BrowserPanel = lazy(() =>
  import("./BrowserPanel").then((module) => ({ default: module.BrowserPanel })),
);
const BrowserDock = lazy(() =>
  import("./BrowserDock").then((module) => ({ default: module.BrowserDock })),
);

function ChatPaneFallback({ label = "加载中..." }: { label?: string }) {
  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        color: "var(--text-muted)",
        fontSize: 13,
      }}
    >
      {label}
    </div>
  );
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
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [showBrowserPanel, setShowBrowserPanel] = useState(false);
  const [dockedBrowsers, setDockedBrowsers] = useState<
    Map<string, { sessionId: string; url: string | null; state: string }>
  >(new Map());
  const browserPanel = useDockedBrowserPanel("nezha.chat.browserPanelWidth");
  const activeSessionIdRef = useRef(activeSessionId);
  activeSessionIdRef.current = activeSessionId;

  useEffect(() => {
    const unlisten = listen<BrowserStatus>("browser-status", (event) => {
      const { sessionId, state, url } = event.payload;
      if (state === "minimized" || state === "page_closed") {
        setDockedBrowsers((prev) => {
          const next = new Map(prev);
          next.set(sessionId, { sessionId, url: url ?? null, state });
          return next;
        });
        if (sessionId === activeSessionIdRef.current) {
          setShowBrowserPanel(false);
        }
      } else if (state === "closed") {
        setDockedBrowsers((prev) => {
          if (!prev.has(sessionId)) return prev;
          const next = new Map(prev);
          next.delete(sessionId);
          return next;
        });
      } else if (sessionId === activeSessionIdRef.current && state !== "page_closed") {
        setShowBrowserPanel(true);
      }
    });

    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);

  const handleMinimizeBrowser = useCallback(() => {
    if (!activeSessionId) return;
    invoke("browser_minimize", { sessionId: activeSessionId }).catch(console.error);
    setShowBrowserPanel(false);
  }, [activeSessionId]);

  const handleRestoreBrowser = useCallback((sessionId: string) => {
    invoke("browser_restore", { sessionId })
      .then(() => {
        setActiveSessionId(sessionId);
        setShowBrowserPanel(true);
      })
      .catch(console.error);
  }, []);

  const handleCloseDockedBrowser = useCallback((sessionId: string) => {
    invoke("browser_stop", { sessionId })
      .then(() => {
        setDockedBrowsers((prev) => {
          const next = new Map(prev);
          next.delete(sessionId);
          return next;
        });
      })
      .catch(console.error);
  }, []);

  const handleReopenBrowser = useCallback(() => {
    if (!activeSessionId) return;
    invoke("browser_reopen", { sessionId: activeSessionId }).catch(console.error);
  }, [activeSessionId]);

  const dockedSessions = useMemo(() => Array.from(dockedBrowsers.values()), [dockedBrowsers]);

  return (
    <div className="nezha-chat-home" style={s.chatHomeBody}>
      <ChatSessionSidebar
        activeSessionId={activeSessionId}
        onActiveSessionChange={setActiveSessionId}
        showBrowserButton
        onToggleBrowser={() => setShowBrowserPanel((v) => !v)}
      />

      <div style={s.chatMainPane}>
        <Suspense fallback={<ChatPaneFallback label="聊天加载中..." />}>
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
        </Suspense>
      </div>

      {showBrowserPanel && (
        <div
          className="nezha-brand-surface"
          style={{
            position: "relative",
            display: "flex",
            flexShrink: 0,
            borderRadius: 18,
            overflow: "hidden",
          }}
        >
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
          <Suspense fallback={<ChatPaneFallback label="浏览器加载中..." />}>
            <BrowserPanel
              sessionId={activeSessionId}
              width={browserPanel.effectiveWidth}
              active={showBrowserPanel}
              expanded={browserPanel.expanded}
              onToggleExpanded={browserPanel.toggleExpanded}
              onClose={() => setShowBrowserPanel(false)}
              onMinimize={handleMinimizeBrowser}
              onReopen={handleReopenBrowser}
            />
          </Suspense>
        </div>
      )}

      {showSettings && (
        <Suspense fallback={null}>
          <AppSettingsDialog
            isDark={isDark}
            themeMode={themeMode}
            systemPrefersDark={systemPrefersDark}
            onThemeModeChange={onThemeModeChange}
            initialTab="aha"
            onClose={() => setShowSettings(false)}
          />
        </Suspense>
      )}

      {dockedSessions.length > 0 && (
        <Suspense fallback={null}>
          <BrowserDock
            sessions={dockedSessions}
            onRestore={handleRestoreBrowser}
            onClose={handleCloseDockedBrowser}
          />
        </Suspense>
      )}
    </div>
  );
}

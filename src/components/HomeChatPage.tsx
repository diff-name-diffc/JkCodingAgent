import { lazy, Suspense, useEffect, useRef, useState } from "react";
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
  const browserPanel = useDockedBrowserPanel("nezha.chat.browserPanelWidth");
  const activeSessionIdRef = useRef(activeSessionId);
  activeSessionIdRef.current = activeSessionId;

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
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);

  return (
    <div style={s.chatHomeBody}>
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
          <Suspense fallback={<ChatPaneFallback label="浏览器加载中..." />}>
            <BrowserPanel
              sessionId={activeSessionId}
              width={browserPanel.effectiveWidth}
              active={showBrowserPanel}
              expanded={browserPanel.expanded}
              onToggleExpanded={browserPanel.toggleExpanded}
              onClose={() => setShowBrowserPanel(false)}
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
    </div>
  );
}

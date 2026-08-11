import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { BrowserStatus } from "../types";
import { useDockedBrowserPanel } from "../hooks/useDockedBrowserPanel";
import { MarkdownLinkProvider } from "./markdown/MarkdownLinkContext";
import { ChatPageV2 } from "./chat-page-v2";

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
  return <div className="ai-home-chat-fallback">{label}</div>;
}

export function HomeChatPage() {
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

  const handleMinimizeBrowser = useCallback(async () => {
    if (!activeSessionId) return;
    await invoke("browser_minimize", { sessionId: activeSessionId });
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

  const handleReopenBrowser = useCallback(async () => {
    if (!activeSessionId) return;
    await invoke("browser_reopen", { sessionId: activeSessionId });
    setShowBrowserPanel(true);
  }, [activeSessionId]);

  const handleOpenMarkdownLink = useCallback(
    async (url: string) => {
      if (!activeSessionId) return;
      setShowBrowserPanel(true);
      try {
        await invoke("browser_navigate", {
          sessionId: activeSessionId,
          url,
          projectPath: null,
        });
      } catch (error) {
        console.error("CloakBrowser 打开链接失败:", error);
      }
    },
    [activeSessionId],
  );

  const dockedSessions = useMemo(() => Array.from(dockedBrowsers.values()), [dockedBrowsers]);

  return (
    <div className="ai-home-chat nezha-chat-home">
      <MarkdownLinkProvider onOpenUrl={handleOpenMarkdownLink}>
        <ChatPageV2
          sessionId={activeSessionId}
          onSessionChange={setActiveSessionId}
          onOpenSettings={() => setShowSettings(true)}
        />
      </MarkdownLinkProvider>

      {showBrowserPanel && (
        <div className="ai-home-chat-browser nezha-brand-surface">
          <div className="ai-home-chat-resizer" onMouseDown={browserPanel.handleResizeStart} />
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
            initialTab="providers"
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

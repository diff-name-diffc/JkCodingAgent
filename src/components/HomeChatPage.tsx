import { lazy, Suspense, useCallback, useMemo, useState } from "react";
import type { ModelCategory } from "../types";
import { useDockedBrowserPanel } from "../hooks/useDockedBrowserPanel";
import { useSplitterKeyboard } from "../hooks/use-splitter-keyboard";
import { useBrowserSessionDock } from "../hooks/useBrowserSessionDock";
import { useChatSessionsQuery } from "../hooks/use-chat-queries";
import { extractMcpToolNames, trimMcpStatusToTools } from "../lib/mcp-category-tools";
import { useAhaSettingsStore } from "./settings/use-aha-settings";
import { MarkdownLinkProvider } from "./markdown/MarkdownLinkContext";
import { ChatPageV2 } from "./chat-page-v2";
import { useGlobalMcpStatus } from "../hooks/use-mcp-status";

const AppSettingsDialog = lazy(() =>
  import("./AppSettingsDialog").then((module) => ({ default: module.AppSettingsDialog })),
);
const McpStatusDialog = lazy(() =>
  import("./McpStatusDialog").then((module) => ({ default: module.McpStatusDialog })),
);
const BrowserPanel = lazy(() =>
  import("./browser/BrowserPanel").then((module) => ({ default: module.BrowserPanel })),
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
  const [settingsInitialTab, setSettingsInitialTab] = useState("providers");
  // 「配置模型」深链携带的目标分类（UI-25 遗留）；null 走 ProvidersPage 缺省。
  const [settingsInitialCategory, setSettingsInitialCategory] = useState<ModelCategory | null>(null);
  const [showMcpStatus, setShowMcpStatus] = useState(false);
  // 全局 MCP 状态快照：所有聊天会话共享；聊天界面实际只展示当前会话
  // 所属分类显式配置的子集（见下方 chatMcpStatus 派生）。
  const {
    status: mcpStatus,
    checking: mcpChecking,
    refresh: refreshMcpStatus,
  } = useGlobalMcpStatus(true);
  // 分类级工具门禁（镜像后端 build_plain_chat_agent 的叠加语义）：
  // 会话挂分类 → 用分类配置的 allowedTools；无分类 → 回退设置级
  // settings.chat.allowedTools。仅其中的 mcp__ 名字决定 MCP 可见性。
  const { settings, chatCategoryConfigs } = useAhaSettingsStore();
  const sessionsQuery = useChatSessionsQuery(undefined, true);
  const activeSessionCategory = useMemo(() => {
    const sessions = sessionsQuery.data ?? [];
    return sessions.find((session) => session.id === activeSessionId)?.category ?? "";
  }, [sessionsQuery.data, activeSessionId]);
  const { configuredMcpNames, activeCategoryName } = useMemo(() => {
    let allowedTools: string[] = settings?.chat.allowedTools ?? [];
    let categoryName: string | null = null;
    if (activeSessionCategory) {
      const config = chatCategoryConfigs.find((item) => item.categoryId === activeSessionCategory);
      if (config) {
        allowedTools = config.allowedTools;
        categoryName = config.categoryName;
      }
    }
    return {
      configuredMcpNames: extractMcpToolNames(allowedTools),
      activeCategoryName: categoryName,
    };
  }, [settings, chatCategoryConfigs, activeSessionCategory]);
  const mcpConfigured = configuredMcpNames.size > 0;
  const chatMcpStatus = useMemo(
    () => trimMcpStatusToTools(mcpStatus, configuredMcpNames),
    [mcpStatus, configuredMcpNames],
  );
  const [showBrowserPanel, setShowBrowserPanel] = useState(false);
  const browserPanel = useDockedBrowserPanel("nezha.chat.browserPanelWidth");
  // 浏览器面板宽度把手键盘化（UI-23c）：右停靠面板，ArrowLeft 加宽（invert）。
  const browserResizerKeyboard = useSplitterKeyboard({
    orientation: "vertical",
    mode: "px",
    invert: true,
    ariaLabel: "方向键调整浏览器面板宽度，双击恢复默认",
    getValue: () => browserPanel.width,
    getBounds: browserPanel.getWidthBounds,
    getDefaultValue: browserPanel.getDefaultWidth,
    onCommit: browserPanel.commitWidth,
  });
  const {
    dockedSessions,
    minimize: handleMinimizeBrowser,
    restore: handleRestoreBrowser,
    closeDocked: handleCloseDockedBrowser,
    reopen: handleReopenBrowser,
    openUrl: handleOpenMarkdownLink,
  } = useBrowserSessionDock({
    activeSessionId,
    projectPath: null,
    onOpen: useCallback(() => setShowBrowserPanel(true), []),
    onMinimized: useCallback(() => setShowBrowserPanel(false), []),
    onRestoreSession: setActiveSessionId,
  });

  return (
    <div className="ai-home-chat nezha-chat-home">
      <MarkdownLinkProvider onOpenUrl={handleOpenMarkdownLink}>
        <ChatPageV2
          sessionId={activeSessionId}
          onSessionChange={setActiveSessionId}
          mcpStatus={mcpConfigured ? chatMcpStatus : null}
          mcpChecking={mcpChecking}
          onOpenMcpStatus={mcpConfigured ? () => setShowMcpStatus(true) : undefined}
          onOpenSettings={(options) => {
            setSettingsInitialTab("providers");
            setSettingsInitialCategory(options?.providersCategory ?? null);
            setShowSettings(true);
          }}
        />
      </MarkdownLinkProvider>

      {showBrowserPanel && (
        <div className="ai-home-chat-browser nezha-brand-surface">
          <div
            {...browserResizerKeyboard}
            className="ai-home-chat-resizer"
            onMouseDown={browserPanel.handleResizeStart}
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

      {showMcpStatus && mcpConfigured && (
        <Suspense fallback={null}>
          <McpStatusDialog
            scope="global"
            status={chatMcpStatus}
            checking={mcpChecking}
            categoryName={activeCategoryName}
            onRefresh={() => {
              refreshMcpStatus().catch(console.error);
            }}
            onOpenSettings={() => {
              setShowMcpStatus(false);
              setSettingsInitialTab("mcp");
              setSettingsInitialCategory(null);
              setShowSettings(true);
            }}
            onClose={() => setShowMcpStatus(false)}
          />
        </Suspense>
      )}

      {showSettings && (
        <Suspense fallback={null}>
          <AppSettingsDialog
            initialTab={settingsInitialTab}
            initialProvidersCategory={settingsInitialCategory ?? undefined}
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

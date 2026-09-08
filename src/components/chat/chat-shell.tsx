import * as React from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
  DispatcherMessage,
  DispatcherModelConfig,
  DispatcherToolArtifactRef,
  ImageSegment,
  ModelCategory,
  OpenSettingsOptions,
  PythonCodeRunRecord,
  ChatSession,
  ChatCategory,
  SubAgentEvent,
  SubAgentRunTrace,
} from "../../types";
import { useUIStore } from "../../stores/ui-store";
import { useChatModelsQuery, useBindChatModel } from "../../hooks/use-chat-queries";
import { getPurposeBinding } from "../settings/providers/provider-registry";
import {
  entriesForCategory,
  entryLabel,
  findEnabledEntryForConfig,
} from "../settings/providers/model-library";
import { useLiveSessionStateReadonly } from "../dispatcher-chat/useLiveSessionState";
import { useChatShortcuts } from "../../hooks/use-chat-shortcuts";
import { useSessionRequestGuard } from "../../hooks/useSessionRequestGuard";
import { AppLayout } from "../layout/app-layout";
import { Sidebar } from "../layout/sidebar";
import { MessageList } from "./message-list";
import type { ChatEmptyStateContent } from "./chat-empty-content";
import { SessionKeywordBar } from "./session-keyword-bar";
import { PromptInput, type ComposerMode } from "./prompt-input";
import { ArtifactPanel } from "../artifact/artifact-panel";
import { CommandPalette } from "./command-palette";
import { SessionScopeContext } from "./session-scope";
import type { ToolActivityItem } from "../dispatcher-chat/tool-activity";
import {
  getSubAgentSession,
  hydrateSubAgentTrace,
  useSubAgentSessions,
} from "../subAgentEventStore";

/**
 * ChatShell — the orchestrator for the Chat surface.
 *
 * Responsibilities:
 *   - Wire AppLayout + Sidebar + MessageList + PromptInput together.
 *   - Read live streaming state from the dispatcherSessionStore singleton.
 *   - Load chat models via TanStack Query.
 *   - Forward send / stop / resume + messages to the parent adapter, which
 *     uses the useDispatcherActions hook + subscribeDispatcherMessages
 *     so the Tauri Channel streaming pipeline is reused verbatim.
 *
 * Why messages come from the adapter: the streaming pipeline pushes finalized
 * messages through subscribeDispatcherMessages (a frontend singleton pub/sub),
 * not a Tauri event. The adapter owns that subscription and passes the merged
 * array down, so streaming + history stay perfectly in sync. Models, by
 * contrast, are a simple request/response and go through TanStack Query
 * directly.
 */
export interface ChatShellProps {
  /** The active conversation id (also used as the dispatcher workspaceId). */
  sessionId: string | null;
  /** Merged message array (history + finalized streaming turns). */
  messages: DispatcherMessage[];
  /** Conversation list for the sidebar. */
  sessions: ChatSession[];
  categories?: ChatCategory[];
  sessionsLoading?: boolean;
  sessionsError?: string;
  /** 会话搜索/列表错误显式重试（UI-25 登记遗留）；缺省时侧栏不渲染重试按钮。 */
  onSessionsRetry?: () => void;
  searchActive?: boolean;
  onActiveSessionChange: (id: string) => void;
  onNewConversation: () => void;
  /** 在指定分类下新建会话（侧边栏分类行内的 + 按钮）。 */
  onNewSessionInCategory?: (categoryId: string) => void;
  onDeleteSession?: (sessionId: string) => void;
  searchValue: string;
  onSearchChange: (value: string) => void;
  /** 打开设置；可携带深链参数（UI-25：「配置模型」直达具体分类标签）。 */
  onOpenSettings: (options?: OpenSettingsOptions) => void;
  onCreateCategory?: (
    name: string,
    config?: { systemPrompt?: string; allowedTools?: string[] },
  ) => void;
  onRenameCategory?: (categoryId: string, name: string) => void;
  onDeleteCategory?: (categoryId: string) => void;
  onMoveSessionToCategory?: (sessionId: string, categoryId: string) => void;

  /** Composer plumbing — provided by the adapter (existing pipeline). */
  input: string;
  onInputChange: (value: string) => void;
  composerMode: ComposerMode;
  onSend: () => void;
  onStop: () => void;
  /** 停止请求进行中（UI-11）：透传给 PromptInput 的停止按钮 loading 态。 */
  isStopping?: boolean;
  attachments?: ImageSegment[];
  onAttachImages?: (files: File[]) => void;
  onRemoveAttachment?: (id: string) => void;
  onRegenerateFromMessage?: (message: DispatcherMessage) => void;
  onEditMessage?: (message: DispatcherMessage) => void;
  editingMessageId?: string | null;
  onCancelEdit?: () => void;
  composerDisabled?: boolean;

  pythonRunRecords?: Record<string, PythonCodeRunRecord>;
  onRunPython?: (target: {
    messageId: string;
    codeBlockIndex: number;
    code: string;
    codeHash: string;
  }) => void;
  embedded?: boolean;
  projectHeader?: React.ReactNode;
  /** 领域化空态文案（UI-25 A06）：普通聊天 / 项目各自传入，不传则用组件缺省。 */
  emptyState?: ChatEmptyStateContent;
  /**
   * UI-23a：多项目保活下隐藏工作区传 false——不注册全局快捷键（消除多实例
   * 叠加触发）、不渲染命令面板（其 open 态来自全局 store，多份渲染会重叠）。
   */
  enabled?: boolean;
  /**
   * UI-23a：布局根节点 ref。把「聚焦输入框」等 DOM 查询限定在本 shell 子树内，
   * 避免保活隐藏实例被全局 querySelector 命中。父级可持有同一 ref 复用。
   */
  containerRef?: React.RefObject<HTMLDivElement | null>;
}

export function ChatShell({
  sessionId,
  messages,
  sessions,
  categories = [],
  sessionsLoading,
  sessionsError,
  onSessionsRetry,
  searchActive = false,
  onActiveSessionChange,
  onNewConversation,
  onNewSessionInCategory,
  onDeleteSession,
  searchValue,
  onSearchChange,
  onOpenSettings,
  onCreateCategory,
  onRenameCategory,
  onDeleteCategory,
  onMoveSessionToCategory,
  input,
  onInputChange,
  composerMode,
  onSend,
  onStop,
  isStopping = false,
  attachments,
  onAttachImages,
  onRemoveAttachment,
  onRegenerateFromMessage,
  onEditMessage,
  editingMessageId,
  onCancelEdit,
  composerDisabled = false,
  pythonRunRecords,
  onRunPython,
  embedded = false,
  projectHeader,
  emptyState,
  enabled = true,
  containerRef,
}: ChatShellProps) {
  const setArtifactPanelOpen = useUIStore((s) => s.setArtifactPanelOpen);
  const artifactPanelOpen = useUIStore((s) => s.artifactPanelOpen);
  const toggleSidebar = useUIStore((s) => s.toggleSidebar);
  const commandPaletteOpen = useUIStore((s) => s.commandPaletteOpen);
  const setCommandPaletteOpen = useUIStore((s) => s.setCommandPaletteOpen);
  const toggleCommandPalette = useUIStore((s) => s.toggleCommandPalette);
  const [selectedArtifact, setSelectedArtifact] = React.useState<DispatcherToolArtifactRef | null>(
    null,
  );
  const [selectedSubAgentToolCallId, setSelectedSubAgentToolCallId] = React.useState<string | null>(
    null,
  );
  const [traceLoading, setTraceLoading] = React.useState(false);
  const [traceError, setTraceError] = React.useState<string | null>(null);
  // 会话切换时让进行中的轨迹请求失效（requestId 范式见 useSessionRequestGuard）。
  const traceGuard = useSessionRequestGuard(sessionId);

  React.useEffect(() => {
    setSelectedArtifact(null);
    setSelectedSubAgentToolCallId(null);
    setTraceLoading(false);
    setTraceError(null);
  }, [sessionId]);

  const modelsQuery = useChatModelsQuery();
  const bindChatModel = useBindChatModel();
  const settings = modelsQuery.data;
  // 聊天输入框可选模型与设置页「聊天主模型」共用统一数据源：模型库 text 分类条目。
  const chatModelEntries = React.useMemo(
    () => entriesForCategory(settings?.modelLibrary ?? [], "text", { enabledOnly: true }),
    [settings],
  );
  const chatBinding = settings ? getPurposeBinding(settings, "chatChat") : null;
  const activeChatEntry = findEnabledEntryForConfig(settings?.modelLibrary ?? [], chatBinding);
  const activeChatLabel =
    (activeChatEntry ? entryLabel(activeChatEntry) : "") ||
    chatBinding?.model ||
    chatBinding?.url ||
    undefined;
  const liveState = useLiveSessionStateReadonly(sessionId);
  const subAgentSessions = useSubAgentSessions(sessionId ?? "");
  const selectedSubAgent = selectedSubAgentToolCallId
    ? (subAgentSessions[selectedSubAgentToolCallId] ?? null)
    : null;

  // 会话关键词展示在聊天界面顶部。
  const activeSessionKeywords = React.useMemo(() => {
    if (!sessionId) return [] as string[];
    return sessions.find((session) => session.id === sessionId)?.keywords ?? [];
  }, [sessions, sessionId]);
  const internalShellRef = React.useRef<HTMLDivElement>(null);
  const shellRef = containerRef ?? internalShellRef;
  const focusPrompt = React.useCallback(() => {
    // 作用域化查询（UI-23a）：多项目保活时页面存在多套聊天 DOM，全局
    // document.querySelector 会聚焦到隐藏工作区的输入框。
    const textarea = shellRef.current?.querySelector<HTMLTextAreaElement>(
      'textarea[aria-label="消息输入框"]',
    );
    textarea?.focus();
  }, [shellRef]);

  // Mod+Shift+A（UI-23d）：开/关 Artifact 详情面板；无选中详情时 no-op，
  // 不制造「打开了空面板」的伪状态。打开瞬间记录触发点，关闭时还原焦点。
  const artifactRestoreFocusRef = React.useRef<HTMLElement | null>(null);
  const handleToggleArtifactPanel = React.useCallback(() => {
    if (artifactPanelOpen) {
      setArtifactPanelOpen(false);
      artifactRestoreFocusRef.current?.focus();
      artifactRestoreFocusRef.current = null;
      return;
    }
    if (!selectedArtifact && !selectedSubAgentToolCallId) return;
    artifactRestoreFocusRef.current = document.activeElement as HTMLElement | null;
    setArtifactPanelOpen(true);
  }, [artifactPanelOpen, selectedArtifact, selectedSubAgentToolCallId, setArtifactPanelOpen]);

  // Global shortcuts. Actions stay in the parent adapter; this shell only
  // coordinates UI state and focus. 隐藏工作区（enabled=false）不注册（UI-23a）。
  useChatShortcuts(
    {
      onToggleCommandPalette: toggleCommandPalette,
      onNewConversation,
      onToggleSidebar: toggleSidebar,
      onFocusPrompt: focusPrompt,
      onCloseArtifact: () => setArtifactPanelOpen(false),
      onToggleArtifactPanel: handleToggleArtifactPanel,
    },
    { enabled },
  );

  const handleCopyMessage = React.useCallback((text: string) => {
    void navigator.clipboard.writeText(text);
  }, []);

  // UI-25 第四批遗留：「配置模型」深链统一回调（run 级 runError 块 + 工具级
  // errorText 卡片共用）。onOpenSettings 在上游调用方为内联箭头（身份不稳定），
  // 直接闭包依赖会随上游重渲染击穿 MessageItem 的 React.memo——按既有 ref
  // 存最新值模式（keyboard-bindings 同款），回调身份恒定。
  const onOpenSettingsRef = React.useRef(onOpenSettings);
  React.useEffect(() => {
    onOpenSettingsRef.current = onOpenSettings;
  }, [onOpenSettings]);
  const handleConfigureModel = React.useCallback((category?: ModelCategory) => {
    onOpenSettingsRef.current({ providersCategory: category ?? "text" });
  }, []);

  const handleOpenArtifact = React.useCallback(
    (artifact: DispatcherToolArtifactRef) => {
      traceGuard.begin(); // 使进行中的轨迹请求失效
      setSelectedArtifact(artifact);
      setSelectedSubAgentToolCallId(null);
      setTraceLoading(false);
      setTraceError(null);
      setArtifactPanelOpen(true);
    },
    [setArtifactPanelOpen, traceGuard],
  );

  const handleOpenSubAgent = React.useCallback(
    async (tool: ToolActivityItem) => {
      if (!sessionId) return;
      const requestId = traceGuard.begin();
      setSelectedArtifact(null);
      setSelectedSubAgentToolCallId(tool.id);
      setTraceError(null);
      setTraceLoading(false);
      setArtifactPanelOpen(true);

      if (getSubAgentSession(sessionId, tool.id)) return;
      if (tool.status === "running") {
        setTraceLoading(true);
        return;
      }
      setTraceLoading(true);
      try {
        const trace = await invoke<SubAgentRunTrace | null>("sub_agent_get_run_trace", {
          workspaceId: sessionId,
          toolCallId: tool.id,
        });
        if (traceGuard.isStale(requestId)) return;
        if (!trace) {
          setTraceError("该任务执行时未记录轨迹。");
          return;
        }
        const parsed: unknown = JSON.parse(trace.eventsJson);
        if (!Array.isArray(parsed)) {
          throw new Error("执行轨迹数据格式无效");
        }
        const hydrated = hydrateSubAgentTrace(
          sessionId,
          tool.id,
          parsed as SubAgentEvent[],
          trace.model,
        );
        if (!hydrated) throw new Error("执行轨迹为空");
      } catch (error) {
        if (traceGuard.isStale(requestId)) return;
        setTraceError(error instanceof Error ? error.message : String(error));
      } finally {
        if (!traceGuard.isStale(requestId)) setTraceLoading(false);
      }
    },
    [sessionId, setArtifactPanelOpen, traceGuard],
  );

  return (
    <SessionScopeContext.Provider value={sessionId}>
    <AppLayout
      containerRef={shellRef}
      chatHeader={projectHeader}
      artifactOverlay={embedded}
      sidebar={
        embedded ? undefined : (
          <Sidebar
            sessions={sessions}
            categories={categories}
            activeSessionId={sessionId}
            onActiveSessionChange={onActiveSessionChange}
            onNewSessionInCategory={onNewSessionInCategory}
            onDeleteSession={onDeleteSession}
            searchValue={searchValue}
            onSearchChange={onSearchChange}
            onOpenSettings={onOpenSettings}
            onCreateCategory={onCreateCategory}
            onRenameCategory={onRenameCategory}
            onDeleteCategory={onDeleteCategory}
            onMoveSessionToCategory={onMoveSessionToCategory}
            loading={sessionsLoading}
            error={sessionsError}
            onRetry={onSessionsRetry}
            searchActive={searchActive}
          />
        )
      }
      chatFooter={
        <PromptInput
          value={input}
          onValueChange={onInputChange}
          mode={composerMode}
          onSend={onSend}
          onStop={onStop}
          stopping={isStopping}
          attachments={attachments}
          onAttachImages={onAttachImages}
          onRemoveAttachment={onRemoveAttachment}
          editing={Boolean(editingMessageId)}
          onCancelEdit={onCancelEdit}
          disabled={composerDisabled}
          models={chatModelEntries}
          activeEntryId={activeChatEntry?.id}
          activeLabel={activeChatLabel}
          onSelectModel={(entryId) => {
            const entry = chatModelEntries.find((item) => item.id === entryId);
            if (entry) bindChatModel.mutate(entry);
          }}
          onConfigureModel={handleConfigureModel}
        />
      }
      artifactPanel={
        // 门控（UI-09 遗留领取）：无详情内容或工作区隐藏（保活多项目）时不
        // 提供面板——AppLayout 的 Sheet portal 挂 body，隐藏 pane 若继续渲染
        // 会带着全局 artifactPanelOpen 弹出空抽屉；同时使会话切换清空内容后
        // 详情面自动收起（旧覆盖层残留「暂无详情」空壳的过渡态一并消除）。
        enabled && (selectedArtifact || selectedSubAgentToolCallId) ? (
          <ArtifactPanel
            title={selectedSubAgentToolCallId ? "子智能体执行轨迹" : "详情"}
            workspaceId={sessionId}
            artifact={selectedArtifact}
            subAgentSession={selectedSubAgent}
            traceLoading={traceLoading}
            traceError={traceError}
          />
        ) : undefined
      }
    >
      {activeSessionKeywords.length > 0 && (
        <SessionKeywordBar keywords={activeSessionKeywords} />
      )}
      <MessageList
        sessionId={sessionId}
        messages={messages}
        liveState={liveState}
        pythonRunRecords={pythonRunRecords}
        onRunPython={onRunPython}
        onCopyMessage={handleCopyMessage}
        onRegenerateFromMessage={onRegenerateFromMessage}
        onEditMessage={onEditMessage}
        onOpenArtifact={handleOpenArtifact}
        onOpenSubAgent={handleOpenSubAgent}
        onPickPrompt={(prompt) => onInputChange(prompt)}
        onConfigureModel={handleConfigureModel}
        emptyState={emptyState}
      />
      {enabled && (
        <CommandPalette
          open={commandPaletteOpen}
          sessions={sessions}
          onOpenChange={setCommandPaletteOpen}
          onNewConversation={onNewConversation}
          onSelectSession={onActiveSessionChange}
          onFocusPrompt={focusPrompt}
          onToggleSidebar={toggleSidebar}
          onOpenSettings={onOpenSettings}
        />
      )}
    </AppLayout>
    </SessionScopeContext.Provider>
  );
}

export type { DispatcherModelConfig };

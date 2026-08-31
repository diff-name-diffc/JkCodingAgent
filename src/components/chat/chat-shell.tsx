import * as React from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
  DispatcherMessage,
  DispatcherModelConfig,
  DispatcherToolArtifactRef,
  ImageSegment,
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
import { AppLayout } from "../layout/app-layout";
import { Sidebar } from "../layout/sidebar";
import { MessageList } from "./message-list";
import { SessionKeywordBar } from "./session-keyword-bar";
import { PromptInput, type ComposerMode } from "./prompt-input";
import { ArtifactPanel } from "../artifact/artifact-panel";
import { CommandPalette } from "./command-palette";
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
  searchActive?: boolean;
  onActiveSessionChange: (id: string) => void;
  onNewConversation: () => void;
  /** 在指定分类下新建会话（侧边栏分类行内的 + 按钮）。 */
  onNewSessionInCategory?: (categoryId: string) => void;
  onDeleteSession?: (sessionId: string) => void;
  searchValue: string;
  onSearchChange: (value: string) => void;
  onOpenSettings: () => void;
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
}

export function ChatShell({
  sessionId,
  messages,
  sessions,
  categories = [],
  sessionsLoading,
  sessionsError,
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
}: ChatShellProps) {
  const setArtifactPanelOpen = useUIStore((s) => s.setArtifactPanelOpen);
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
  const traceRequestRef = React.useRef(0);

  // 会话切换时让进行中的轨迹请求失效（requestId 比对见 handleOpenSubAgent）。
  React.useEffect(() => {
    traceRequestRef.current += 1;
  }, [sessionId]);

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
  const focusPrompt = React.useCallback(() => {
    const textarea = document.querySelector<HTMLTextAreaElement>(
      'textarea[aria-label="消息输入框"]',
    );
    textarea?.focus();
  }, []);

  // Global shortcuts. Actions stay in the parent adapter; this shell only
  // coordinates UI state and focus.
  useChatShortcuts({
    onToggleCommandPalette: toggleCommandPalette,
    onNewConversation,
    onToggleSidebar: toggleSidebar,
    onFocusPrompt: focusPrompt,
    onCloseArtifact: () => setArtifactPanelOpen(false),
  });

  const handleCopyMessage = React.useCallback((text: string) => {
    void navigator.clipboard.writeText(text);
  }, []);

  const handleOpenArtifact = React.useCallback(
    (artifact: DispatcherToolArtifactRef) => {
      traceRequestRef.current += 1;
      setSelectedArtifact(artifact);
      setSelectedSubAgentToolCallId(null);
      setTraceLoading(false);
      setTraceError(null);
      setArtifactPanelOpen(true);
    },
    [setArtifactPanelOpen],
  );

  const handleOpenSubAgent = React.useCallback(
    async (tool: ToolActivityItem) => {
      if (!sessionId) return;
      const requestId = ++traceRequestRef.current;
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
        if (requestId !== traceRequestRef.current) return;
        if (!trace) {
          setTraceError("该任务执行时未记录轨迹。");
          return;
        }
        const parsed: unknown = JSON.parse(trace.eventsJson);
        if (!Array.isArray(parsed)) {
          throw new Error("执行轨迹数据格式无效");
        }
        const hydrated = hydrateSubAgentTrace(sessionId, tool.id, parsed as SubAgentEvent[]);
        if (!hydrated) throw new Error("执行轨迹为空");
      } catch (error) {
        if (requestId !== traceRequestRef.current) return;
        setTraceError(error instanceof Error ? error.message : String(error));
      } finally {
        if (requestId === traceRequestRef.current) setTraceLoading(false);
      }
    },
    [sessionId, setArtifactPanelOpen],
  );

  return (
    <AppLayout
      chatHeader={projectHeader}
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
        />
      }
      artifactPanel={
        <ArtifactPanel
          title={selectedSubAgentToolCallId ? "子智能体执行轨迹" : "详情"}
          workspaceId={sessionId}
          artifact={selectedArtifact}
          subAgentSession={selectedSubAgent}
          traceLoading={traceLoading}
          traceError={traceError}
        />
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
      />
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
    </AppLayout>
  );
}

export type { DispatcherModelConfig };

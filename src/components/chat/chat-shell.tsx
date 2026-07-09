import * as React from "react";
import type {
  DispatcherMessage,
  DispatcherModelConfig,
  DispatcherToolArtifactRef,
  PythonCodeRunRecord,
  ThemeMode,
  ChatSession,
  ChatCategory,
} from "../../types";
import { useUIStore } from "../../stores/ui-store";
import { useChatModelsQuery, useSetActiveChatModel } from "../../hooks/use-chat-queries";
import { useLiveSessionState } from "../../hooks/use-live-session-state";
import { useChatShortcuts } from "../../hooks/use-chat-shortcuts";
import { AppLayout } from "../layout/app-layout";
import { Sidebar } from "../layout/sidebar";
import { MessageList } from "./message-list";
import { PromptInput, type ComposerMode } from "./prompt-input";
import { ThemeToggle } from "./theme-toggle";
import { ArtifactPanel } from "../artifact/artifact-panel";
import { DispatchApprovalPanel } from "./dispatch-approval-panel";
import type { PendingDispatchApproval } from "../dispatcherSessionStore";
import { CommandPalette } from "./command-palette";
import type { ToolActivityItem } from "../ToolActivityBubble";
import {
  extractAgentIdsFromToolInput,
  useSubAgentSessions,
} from "../subAgentEventStore";

/**
 * ChatShell — the orchestrator for the refactored Chat surface.
 *
 * Responsibilities:
 *   - Wire AppLayout + Sidebar + MessageList + PromptInput together.
 *   - Read live streaming state from the existing dispatcherSessionStore
 *     singleton (unchanged pipeline).
 *   - Load chat models via TanStack Query.
 *   - Forward send / stop / resume + messages to the parent adapter, which
 *     uses the existing useDispatcherActions hook + subscribeDispatcherMessages
 *     so the Tauri Channel streaming pipeline is reused verbatim.
 *
 * Why messages come from the adapter: the streaming pipeline pushes finalized
 * messages through subscribeDispatcherMessages (a frontend singleton pub/sub),
 * not a Tauri event. The adapter owns that subscription and passes the merged
 * array down, so streaming + history stay perfectly in sync with the legacy
 * surface. Models, by contrast, are a simple request/response and go through
 * TanStack Query directly.
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
  searchActive?: boolean;
  onActiveSessionChange: (id: string) => void;
  onNewConversation: () => void;
  searchValue: string;
  onSearchChange: (value: string) => void;
  onOpenSettings: () => void;
  onCreateCategory?: (name: string, config?: { systemPrompt?: string; allowedTools?: string[] }) => void;
  onRenameCategory?: (categoryId: string, name: string) => void;
  onDeleteCategory?: (categoryId: string) => void;
  onMoveSessionToCategory?: (sessionId: string, categoryId: string) => void;

  /** Composer plumbing — provided by the adapter (existing pipeline). */
  input: string;
  onInputChange: (value: string) => void;
  composerMode: ComposerMode;
  onSend: () => void;
  onStop: () => void;
  onResume?: () => void;
  onRegenerate?: () => void;
  onApproveDispatch?: (dispatchId: string, taskPrompt: string) => void;
  onRejectDispatch?: (dispatchId: string) => void;

  /** Theme. The actual html.dark application stays in App.tsx. */
  theme: ThemeMode;
  isDark: boolean;
  onThemeChange: (mode: ThemeMode) => void;

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
  searchActive = false,
  onActiveSessionChange,
  onNewConversation,
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
  onResume,
  onRegenerate,
  onApproveDispatch,
  onRejectDispatch,
  theme,
  isDark,
  onThemeChange,
  pythonRunRecords,
  onRunPython,
  embedded = false,
  projectHeader,
}: ChatShellProps) {
  const setActiveConversationId = useUIStore((s) => s.setActiveConversationId);
  const setArtifactPanelOpen = useUIStore((s) => s.setArtifactPanelOpen);
  const toggleSidebar = useUIStore((s) => s.toggleSidebar);
  const commandPaletteOpen = useUIStore((s) => s.commandPaletteOpen);
  const setCommandPaletteOpen = useUIStore((s) => s.setCommandPaletteOpen);
  const toggleCommandPalette = useUIStore((s) => s.toggleCommandPalette);
  const [selectedArtifact, setSelectedArtifact] =
    React.useState<DispatcherToolArtifactRef | null>(null);
  const [selectedSubAgentId, setSelectedSubAgentId] =
    React.useState<string | null>(null);

  // Keep the UI store's active conversation in sync (for the artifact panel
  // and other consumers). Pure UI state — no business logic.
  React.useEffect(() => {
    setActiveConversationId(sessionId);
  }, [sessionId, setActiveConversationId]);

  React.useEffect(() => {
    setSelectedArtifact(null);
    setSelectedSubAgentId(null);
  }, [sessionId]);

  const modelsQuery = useChatModelsQuery();
  const selectModel = useSetActiveChatModel();
  const liveState = useLiveSessionState(sessionId);
  const subAgentSessions = useSubAgentSessions(sessionId ?? "");
  const selectedSubAgent = selectedSubAgentId ? subAgentSessions[selectedSubAgentId] ?? null : null;
  const currentPendingDispatch: PendingDispatchApproval | null =
    liveState?.pendingDispatches[0] ?? null;
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
      setSelectedArtifact(artifact);
      setSelectedSubAgentId(null);
      setArtifactPanelOpen(true);
    },
    [setArtifactPanelOpen],
  );

  const handleOpenSubAgent = React.useCallback(
    (tool: ToolActivityItem) => {
      const agentId = extractAgentIdsFromToolInput(tool.input);
      if (!agentId) return;
      const session = subAgentSessions[agentId];
      if (!session) return;
      setSelectedArtifact(null);
      setSelectedSubAgentId(agentId);
      setArtifactPanelOpen(true);
    },
    [setArtifactPanelOpen, subAgentSessions],
  );

  return (
    <AppLayout
      embedded={embedded}
      chatHeader={projectHeader}
      sidebar={
        embedded ? undefined : (
        <Sidebar
          sessions={sessions}
          categories={categories}
          activeSessionId={sessionId}
          onActiveSessionChange={onActiveSessionChange}
          onNewConversation={onNewConversation}
          searchValue={searchValue}
          onSearchChange={onSearchChange}
          onOpenSettings={onOpenSettings}
          onCreateCategory={onCreateCategory}
          onRenameCategory={onRenameCategory}
          onDeleteCategory={onDeleteCategory}
          onMoveSessionToCategory={onMoveSessionToCategory}
          loading={sessionsLoading}
          searchActive={searchActive}
          footer={
            <ThemeToggle
              theme={theme}
              isDark={isDark}
              onChange={onThemeChange}
            />
          }
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
          onResume={onResume}
          models={modelsQuery.data}
          onSelectModel={(index) => selectModel.mutate(index)}
        />
      }
      artifactPanel={
        <ArtifactPanel
          title={selectedSubAgent ? "子智能体执行轨迹" : "详情"}
          workspaceId={sessionId}
          artifact={selectedArtifact}
          subAgentSession={selectedSubAgent}
        />
      }
    >
      <MessageList
        messages={messages}
        liveState={liveState}
        pythonRunRecords={pythonRunRecords}
        onRunPython={onRunPython}
        onCopyMessage={handleCopyMessage}
        onRegenerate={onRegenerate}
        onOpenArtifact={handleOpenArtifact}
        onOpenSubAgent={handleOpenSubAgent}
        onPickPrompt={(prompt) => onInputChange(prompt)}
      />
      {currentPendingDispatch && onApproveDispatch && onRejectDispatch && (
        <DispatchApprovalPanel
          dispatchId={currentPendingDispatch.dispatchId}
          agent={currentPendingDispatch.agent}
          description={currentPendingDispatch.description}
          taskPrompt={currentPendingDispatch.taskPrompt}
          permissionMode={currentPendingDispatch.permissionMode}
          onApprove={onApproveDispatch}
          onReject={onRejectDispatch}
        />
      )}
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

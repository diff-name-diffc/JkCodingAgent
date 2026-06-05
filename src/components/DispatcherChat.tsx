import {
  useState,
  useRef,
  useEffect,
  useCallback,
  useImperativeHandle,
  forwardRef,
  useMemo,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AgentType,
  ChecklistPlanState,
  DispatcherMessage,
  DispatcherMode,
  DispatcherSessionRuntimeState,
  DispatcherSettings,
  ImageSegment,
  PlanInteraction,
  ProjectMcpStatus,
  PythonCodeRunRecord,
  PythonCodeRunTarget,
  PythonRunEvent,
  SessionKeyword,
  SubProcess,
} from "../types";
import { useDashScopeAsr } from "../hooks/useDashScopeAsr";
import { useDispatcherSessionTokenUsage } from "../hooks/useDispatcherSessionTokenUsage";
import { buildDispatcherDisplayItems } from "./dispatcherChatView";
import { subscribeDispatcherMessages } from "./dispatcherSessionStore";

// Extracted sub-components
import { ChatHeader } from "./dispatcher-chat/ChatHeader";
import { MessageList } from "./dispatcher-chat/MessageListView";
import { ComposerInput } from "./dispatcher-chat/ComposerInput";
import { DispatchApprovalDialog } from "./dispatcher-chat/DispatchApprovalDialog";
import { EmptyConversationLauncher } from "./dispatcher-chat/EmptyConversationLauncher";
import { PythonRunDrawer } from "./dispatcher-chat/PythonRunDrawer";

// Extracted hooks
import { useConversationSearch } from "./dispatcher-chat/useConversationSearch";
import { useLiveSessionState } from "./dispatcher-chat/useLiveSessionState";
import {
  useDispatcherActions,
  type DispatcherChatHandle,
} from "./dispatcher-chat/useDispatcherActions";
import { useDispatcherHandlers } from "./dispatcher-chat/useDispatcherHandlers";

// Extracted utilities
import {
  mergeDispatcherMessages,
  getMcpIndicatorState,
  withLiveElapsed,
} from "./dispatcher-chat/dispatcherChatUtils";

// Re-exports for backward compatibility
export { InteractionDrawer } from "./dispatcher-chat/InteractionDrawer";
export {
  buildPlanQuestionAnswer,
  buildPlanImplementationPrompt,
} from "./dispatcher-chat/dispatcherChatUtils";
export { cleanupDispatcherSession, gcDispatcherSessions } from "./dispatcherSessionStore";
export type { DispatcherChatHandle } from "./dispatcher-chat/useDispatcherActions";

// Extracted styles
import { dispatcherChatStyles as styles } from "./dispatcher-chat/dispatcherChatStyles";

interface DispatcherChatProps {
  sessionId: string;
  conversationKind?: "project" | "chat";
  projectPath?: string;
  mcpStatus?: ProjectMcpStatus | null;
  mcpChecking?: boolean;
  layoutMode?: "single" | "split";
  subProcesses?: SubProcess[];
  onDispatchApproved?: (
    dispatchId: string, agent: AgentType, description: string,
    taskPrompt: string, permissionMode: string, sessionId: string,
  ) => void;
  onDispatchRejected?: (dispatchId: string) => void;
  onDispatchContinue?: (agent: AgentType, text: string, sessionId: string) => void;
  onDispatchExit?: (agent: AgentType, reason: string, sessionId: string) => void;
  onStopActiveRun?: (sessionId: string) => Promise<void>;
  onResumeStoppedRun?: (sessionId: string) => Promise<void>;
  onOpenMcpStatus?: () => void;
  onOpenSettings: () => void;
  onOpenPlanDocument?: (path: string) => void;
  onClosePanel?: () => void;
}

export const DispatcherChat = forwardRef<DispatcherChatHandle, DispatcherChatProps>(
  function DispatcherChat(
    {
      sessionId,
      conversationKind = "project",
      projectPath = "",
      mcpStatus = null,
      mcpChecking = false,
      subProcesses: _subProcesses = [],
      layoutMode,
      onDispatchApproved,
      onDispatchRejected,
      onDispatchContinue,
      onDispatchExit,
      onStopActiveRun,
      onResumeStoppedRun,
      onOpenMcpStatus,
      onOpenSettings,
      onOpenPlanDocument,
      onClosePanel,
    },
    ref,
  ) {
    const isPlainChat = conversationKind === "chat";

    // ── Local state ──────────────────────────────────────────────
    const [messages, setMessages] = useState<DispatcherMessage[]>([]);
    const [input, setInput] = useState("");
    const [attachedImages, setAttachedImages] = useState<ImageSegment[]>([]);
    const [isStopping, setIsStopping] = useState(false);
    const [autoApprove, setAutoApprove] = useState(false);
    const [mode, setMode] = useState<DispatcherMode>("default");
    const [checklist, setChecklist] = useState<ChecklistPlanState | null>(null);
    const [planInteraction, setPlanInteraction] = useState<PlanInteraction | null>(null);
    const [activePlanPath, setActivePlanPath] = useState<string | null>(null);
    const [implementingPlan, setImplementingPlan] = useState(false);
    const [thinkingEnabled, setThinkingEnabled] = useState(false);
    const [pythonDrawerOpen, setPythonDrawerOpen] = useState(false);
    const [pythonRunTarget, setPythonRunTarget] = useState<PythonCodeRunTarget | null>(null);
    const [pythonRunRecords, setPythonRunRecords] = useState<Record<string, PythonCodeRunRecord>>({});
    const [sessionKeywords, setSessionKeywords] = useState<SessionKeyword[]>([]);

    // ── Refs ─────────────────────────────────────────────────────
    const messageListRef = useRef<HTMLDivElement>(null);
    const inputRef = useRef<HTMLTextAreaElement>(null);
    const inputComposingRef = useRef(false);
    const currentSessionIdRef = useRef(sessionId);
    currentSessionIdRef.current = sessionId;
    const historyLoadRef = useRef(0);
    const shouldStickToBottomRef = useRef(true);
    const autoApproveRef = useRef(autoApprove);
    autoApproveRef.current = autoApprove;
    const thinkingEnabledRef = useRef(thinkingEnabled);
    thinkingEnabledRef.current = thinkingEnabled;
    const onDispatchApprovedRef = useRef(onDispatchApproved);
    onDispatchApprovedRef.current = onDispatchApproved;
    const onDispatchContinueRef = useRef(onDispatchContinue);
    onDispatchContinueRef.current = onDispatchContinue;
    const onDispatchExitRef = useRef(onDispatchExit);
    onDispatchExitRef.current = onDispatchExit;

    // ── Extracted hooks ──────────────────────────────────────────
    const { liveState, updateLiveSessionState } = useLiveSessionState(sessionId);
    const {
      entries: sessionTokenUsageEntries,
      refresh: refreshSessionTokenUsage,
      reset: resetSessionTokenUsage,
    } = useDispatcherSessionTokenUsage(sessionId);

    const {
      isLoading, hasPendingRun, streamingSegments, liveThinking, liveToolCalls,
      assistantPlaceholder, runError, pendingDispatches, activeUsageStats,
      activeUsageStatsReceivedAt, usageClockNow,
    } = liveState;

    const scrollMessageListToBottom = useCallback((behavior: ScrollBehavior = "auto") => {
      messageListRef.current?.scrollTo({ top: messageListRef.current.scrollHeight, behavior });
    }, []);

    const actions = useDispatcherActions({
      sessionId, projectPath, isPlainChat, mode, thinkingEnabledRef,
      updateLiveSessionState, scrollMessageListToBottom, currentSessionIdRef,
      refreshSessionTokenUsage, onOpenPlanDocument, autoApproveRef,
      onDispatchApprovedRef, onDispatchContinueRef, onDispatchExitRef,
      setMode, setChecklist, setPlanInteraction, setActivePlanPath,
      shouldStickToBottomRef, setInput, setAttachedImages,
    });

    const search = useConversationSearch({
      messageListRef, inputRef, messages, streamingSegments, liveThinking, assistantPlaceholder,
    });

    // ── Derived state ────────────────────────────────────────────
    const sessionSubProcesses = useMemo(
      () => _subProcesses.filter((sp) => sp.sessionId === sessionId),
      [_subProcesses, sessionId],
    );
    const hasRunningSubProcess = !isPlainChat && sessionSubProcesses.some((sp) => sp.status === "running");
    const hasStoppedSubProcess = !isPlainChat && sessionSubProcesses.some((sp) => sp.status === "stopped");
    const composerMode: "send" | "stop" | "resume" =
      hasPendingRun || isLoading || hasRunningSubProcess ? "stop"
        : hasStoppedSubProcess ? "resume" : "send";
    const displayItems = useMemo(() => buildDispatcherDisplayItems(messages), [messages]);
    const currentPendingDispatch = pendingDispatches[0] ?? null;
    const mcpIndicator = getMcpIndicatorState(mcpStatus, mcpChecking);
    const liveUsageStats = useMemo(
      () => withLiveElapsed(activeUsageStats, activeUsageStatsReceivedAt, usageClockNow),
      [activeUsageStats, activeUsageStatsReceivedAt, usageClockNow],
    );
    const hasLiveSegments = streamingSegments.some((s) => s.text.trim());
    const selectedPythonRun = pythonRunTarget
      ? pythonRunRecords[pythonRunKey(pythonRunTarget.messageId, pythonRunTarget.codeHash)] ?? null
      : null;
    const selectedPythonRunning = selectedPythonRun?.status === "running";
    const isEmpty =
      messages.length === 0 && !hasLiveSegments && !liveThinking &&
      liveToolCalls.length === 0 && !assistantPlaceholder?.trim();

    const voiceInput = useDashScopeAsr({
      workspaceId: sessionId,
      enabled: composerMode !== "stop" && !isStopping,
      onTranscriptReady: async (text) => { await actions.sendUserMessage(text, [], sessionId); },
    });

    const handleRunPython = useCallback(async (target: PythonCodeRunTarget) => {
      setPythonRunTarget(target);
      setPythonDrawerOpen(true);
      try {
        const started = await invoke<PythonCodeRunRecord>("python_runner_start", {
          workspaceId: sessionId,
          messageId: target.messageId,
          codeBlockIndex: target.codeBlockIndex,
          code: target.code,
        });
        setPythonRunRecords((prev) => ({
          ...prev,
          [pythonRunKey(started.messageId, started.codeHash)]: started,
        }));
      } catch (error) {
        console.error("启动 Python 执行失败:", error);
      }
    }, [sessionId]);

    const handleStopPythonRun = useCallback(async (runId: string) => {
      try {
        await invoke("python_runner_stop", { runId });
      } catch (error) {
        console.error("停止 Python 执行失败:", error);
      }
    }, []);

    const handleClearPythonRun = useCallback(async (target: PythonCodeRunTarget) => {
      try {
        await invoke("python_runner_clear_result", {
          workspaceId: sessionId,
          messageId: target.messageId,
          codeBlockIndex: target.codeBlockIndex,
        });
        setPythonRunRecords((prev) => {
          const next = { ...prev };
          delete next[pythonRunKey(target.messageId, target.codeHash)];
          return next;
        });
      } catch (error) {
        console.error("清空 Python 执行结果失败:", error);
      }
    }, [sessionId]);

    // ── Handlers (extracted to useDispatcherHandlers) ────────────
    const handlers = useDispatcherHandlers({
      sessionId, input, attachedImages, isStopping, autoApprove, mode, planInteraction,
      inputRef, inputComposingRef, shouldStickToBottomRef,
      composerMode, isLoading, currentPendingDispatch,
      actions, updateLiveSessionState, voiceInput,
      resetSessionTokenUsage,
      setMessages, setAttachedImages, setIsStopping,
      setAutoApprove, setMode, setChecklist, setPlanInteraction,
      setActivePlanPath, setImplementingPlan, setThinkingEnabled,
      onDispatchApproved, onDispatchRejected, onStopActiveRun, onResumeStoppedRun,
    });

    // ── Effects ──────────────────────────────────────────────────
    useEffect(() => {
      invoke<DispatcherSettings | null>("dispatcher_get_settings")
        .then((s) => { if (s) setAutoApprove(s.autoApproveDispatch); })
        .catch(console.error);
    }, []);

    useEffect(() => {
      const loadId = ++historyLoadRef.current;
      shouldStickToBottomRef.current = true;
      setMessages([]); setIsStopping(false); setChecklist(null);
      setPlanInteraction(null); setActivePlanPath(null); setImplementingPlan(false);
      setPythonRunTarget(null); setPythonRunRecords({});
      invoke<DispatcherMessage[]>("dispatcher_list_messages", { workspaceId: sessionId })
        .then((loaded) => {
          if (currentSessionIdRef.current !== sessionId || historyLoadRef.current !== loadId) return;
          setMessages((prev) =>
            mergeDispatcherMessages(
              loaded.filter((m) => m.workspaceId === sessionId),
              prev.filter((m) => m.workspaceId === sessionId),
            ),
          );
        }).catch(console.error);
      invoke<DispatcherSessionRuntimeState>("dispatcher_get_session_runtime_state", { sessionId })
        .then((state) => {
          if (currentSessionIdRef.current !== sessionId || historyLoadRef.current !== loadId) return;
          setMode(state.mode); setChecklist(state.checklist ?? null);
          setPlanInteraction(state.planInteraction ?? null);
          setActivePlanPath(state.activePlanPath ?? null);
        }).catch(console.error);
    }, [sessionId]);

    useEffect(() => {
      let cancelled = false;
      invoke<PythonCodeRunRecord[]>("python_runner_list_results", { workspaceId: sessionId })
        .then((records) => {
          if (cancelled) return;
          setPythonRunRecords(indexPythonRuns(records));
        })
        .catch(console.error);
      return () => {
        cancelled = true;
      };
    }, [sessionId]);

    useEffect(() => {
      invoke<SessionKeyword[]>("session_get_keywords", { workspaceId: sessionId })
        .then(setSessionKeywords)
        .catch(() => setSessionKeywords([]));
      const unlisten = listen<{ workspaceId: string; keywords: SessionKeyword[] }>(
        "session-keywords-updated",
        (event) => {
          if (event.payload.workspaceId === sessionId) {
            setSessionKeywords(event.payload.keywords);
          }
        },
      );
      return () => {
        unlisten.then((fn) => fn()).catch(() => {});
      };
    }, [sessionId]);

    useEffect(() => {
      const unlisten = listen<PythonRunEvent>("python-run-event", (event) => {
        const payload = event.payload;
        if (payload.workspaceId !== currentSessionIdRef.current) return;
        const record = payload.data.record;
        if (record) {
          setPythonRunRecords((prev) => ({
            ...prev,
            [pythonRunKey(record.messageId, record.codeHash)]: record,
          }));
          return;
        }
        if (payload.event === "output") {
          // Find the record for this run by matching runId across all records
          setPythonRunRecords((prev) => {
            const entry = Object.entries(prev).find(([, r]) => r.runId === payload.runId);
            if (!entry) return prev;
            const [key, existing] = entry;
            return {
              ...prev,
              [key]: {
                ...existing,
                stdout: (payload.data.stdout ?? "") + existing.stdout,
              },
            };
          });
        }
      });

      return () => {
        unlisten.then((fn) => fn()).catch(() => {});
      };
    }, []);

    useEffect(
      () => subscribeDispatcherMessages(sessionId, (incoming) => {
        setMessages((prev) =>
          mergeDispatcherMessages(prev, incoming.filter((m) => m.workspaceId === sessionId)),
        );
      }),
      [sessionId],
    );

    useEffect(() => {
      if (!shouldStickToBottomRef.current) return;
      scrollMessageListToBottom();
    }, [messages, liveThinking, liveToolCalls, runError, scrollMessageListToBottom]);

    useEffect(() => {
      if (!shouldStickToBottomRef.current || (!streamingSegments.length && !assistantPlaceholder)) return;
      const id = requestAnimationFrame(() => scrollMessageListToBottom());
      return () => cancelAnimationFrame(id);
    }, [streamingSegments, assistantPlaceholder, scrollMessageListToBottom]);

    useImperativeHandle(ref, () => ({
      continueWithResult: actions.continueWithResult,
      applyRuntimeState: actions.applyRuntimeState,
    }), [actions.continueWithResult, actions.applyRuntimeState]);

    // ── Render ───────────────────────────────────────────────────
    return (
      <div style={styles.pythonRunnerShell}>
        <div style={styles.container}>
          <ChatHeader
            isPlainChat={isPlainChat} thinkingEnabled={thinkingEnabled} isLoading={isLoading}
            activePlanPath={activePlanPath} autoApprove={autoApprove} mcpIndicator={mcpIndicator}
            hasMessages={messages.length > 0}
            keywords={sessionKeywords}
            onClickKeyword={(kw) => {
              navigator.clipboard.writeText(kw).catch(() => {});
            }}
            searchOpen={search.searchOpen} searchQuery={search.searchQuery}
            matchCount={search.matchCount} activeIndex={search.activeIndex}
            searchInputRef={search.searchInputRef}
            onSearchChange={search.handleSearchChange} onSearchKeyDown={search.handleSearchKeyDown}
            onFocusSearch={search.focusSearch} onCloseSearch={search.closeSearch}
            onMoveSearchMatch={search.moveSearchMatch}
            onToggleAutoApprove={handlers.handleToggleAutoApprove}
            onOpenMcpStatus={() => onOpenMcpStatus?.()}
            onClearHistory={handlers.handleClearHistory}
            onOpenSettings={onOpenSettings} onClosePanel={onClosePanel}
          />
          <MessageList
            displayItems={displayItems} streamingSegments={streamingSegments}
            liveThinking={liveThinking} liveToolCalls={liveToolCalls}
            assistantPlaceholder={assistantPlaceholder} liveUsageStats={liveUsageStats}
            isStreaming={isLoading || hasPendingRun} isEmpty={isEmpty} isPlainChat={isPlainChat}
            runError={runError} sessionId={sessionId} checklist={checklist}
            planInteraction={planInteraction} implementingPlan={implementingPlan}
            messageListRef={messageListRef} onScroll={handlers.handleMessageListScroll}
            onAnswerPlanQuestion={handlers.handleAnswerPlanQuestion}
            onImplementPlan={handlers.handleImplementPlan}
            onImplementPlanWithClearedContext={handlers.handleImplementPlanWithClearedContext}
            onStayInPlanMode={handlers.handleStayInPlanMode}
            onRunPython={handleRunPython}
            pythonRunRecords={pythonRunRecords}
          />
          {isEmpty && !isPlainChat && (
            <EmptyConversationLauncher
              conversationKind={conversationKind} input={input} attachedImages={attachedImages}
              composerMode={composerMode} mode={mode}
              isBusy={isLoading || isStopping} isStopping={isStopping}
              isRecordingVoice={voiceInput.isRecording} autoApprove={autoApprove}
              thinkingEnabled={thinkingEnabled}
              sessionTokenUsages={sessionTokenUsageEntries}
              voiceTranscript={voiceInput.transcript} voiceError={voiceInput.error}
              inputRef={inputRef} layoutMode={layoutMode ?? "split"}
              onChangeInput={setInput} onPaste={handlers.handlePaste}
              onDrop={handlers.handleDrop} onDragOver={handlers.handleDragOver}
              onRemoveImage={handlers.handleRemoveImage}
              onSend={handlers.handleSend} onStop={handlers.onStop} onResume={handlers.onResume}
              onToggleMode={handlers.handleModeToggle} onToggleThinking={handlers.handleToggleThinking}
              onToggleVoiceInput={voiceInput.toggleRecording} onDismissVoiceError={voiceInput.clearError}
              onKeyDown={handlers.handleKeyDown}
              onOpenSettings={onOpenSettings}
              onOpenMcpStatus={() => onOpenMcpStatus?.()}
              onToggleAutoApprove={handlers.handleToggleAutoApprove}
              onCompositionStart={() => { inputComposingRef.current = true; }}
              onCompositionEnd={() => { inputComposingRef.current = false; }}
            />
          )}
          {(!isEmpty || isPlainChat) && (
            <ComposerInput
              isPlainChat={isPlainChat} input={input} attachedImages={attachedImages}
              composerMode={composerMode} mode={mode} thinkingEnabled={thinkingEnabled}
              isComposerBusy={isLoading || isStopping} isStopping={isStopping}
              isRecordingVoice={voiceInput.isRecording} voiceTranscript={voiceInput.transcript}
              voiceError={voiceInput.error} inputRef={inputRef}
              checklist={checklist} planInteraction={planInteraction}
              implementingPlan={implementingPlan} sessionTokenUsageEntries={sessionTokenUsageEntries}
              onChangeInput={setInput} onPaste={handlers.handlePaste}
              onDrop={handlers.handleDrop} onDragOver={handlers.handleDragOver}
              onRemoveImage={handlers.handleRemoveImage}
              onSend={handlers.handleSend} onStop={handlers.onStop} onResume={handlers.onResume}
              onKeyDown={handlers.handleKeyDown}
              onToggleMode={handlers.handleModeToggle} onToggleThinking={handlers.handleToggleThinking}
              onToggleVoiceInput={voiceInput.toggleRecording} onDismissVoiceError={voiceInput.clearError}
              onCompositionStart={() => { inputComposingRef.current = true; }}
              onCompositionEnd={() => { inputComposingRef.current = false; }}
              onAnswerPlanQuestion={handlers.handleAnswerPlanQuestion}
              onImplementPlan={handlers.handleImplementPlan}
              onImplementPlanWithClearedContext={handlers.handleImplementPlanWithClearedContext}
              onStayInPlanMode={handlers.handleStayInPlanMode}
            />
          )}
          {!isPlainChat && currentPendingDispatch && (
            <DispatchApprovalDialog
              dispatchId={currentPendingDispatch.dispatchId}
              agent={currentPendingDispatch.agent} description={currentPendingDispatch.description}
              taskPrompt={currentPendingDispatch.taskPrompt}
              permissionMode={currentPendingDispatch.permissionMode}
              onApprove={handlers.handleApproveDispatch} onReject={handlers.handleRejectDispatch}
            />
          )}
        </div>
        {pythonDrawerOpen && (
          <PythonRunDrawer
            target={pythonRunTarget}
            record={selectedPythonRun}
            running={selectedPythonRunning}
            onClose={() => setPythonDrawerOpen(false)}
            onRun={handleRunPython}
            onStop={handleStopPythonRun}
            onClear={handleClearPythonRun}
          />
        )}
      </div>
    );
  },
);

function pythonRunKey(messageId: string, codeHash: string) {
  return `${messageId}:${codeHash}`;
}

function indexPythonRuns(records: PythonCodeRunRecord[]) {
  return records.reduce<Record<string, PythonCodeRunRecord>>((acc, record) => {
    acc[pythonRunKey(record.messageId, record.codeHash)] = record;
    return acc;
  }, {});
}

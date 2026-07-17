import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { confirm } from "@tauri-apps/plugin-dialog";
import type {
  AgentType,
  AhaSettingsV2,
  AnyContentSegment,
  DispatcherMessage,
  ImageSegment,
  PythonCodeRunRecord,
  PythonCodeRunTarget,
  PythonRunEvent,
  ProjectMcpStatus,
  SubProcess,
} from "../types";
import {
  useChatCategoriesQuery,
  useCreateChatCategory,
  useCreateChatSession,
  useDeleteChatCategory,
  useDeleteChatSession,
  useSetChatSessionCategory,
  useChatSessionUpdates,
  useChatSessionsQuery,
  useSessionSearchQuery,
  useUpdateChatCategory,
} from "../hooks/use-chat-queries";
import { useDispatcherSessionTokenUsage } from "../hooks/useDispatcherSessionTokenUsage";
import { useLiveSessionState } from "./dispatcher-chat/useLiveSessionState";
import {
  useDispatcherActions,
} from "./dispatcher-chat/useDispatcherActions";
import {
  mergeDispatcherMessages,
  getMcpIndicatorState,
} from "./dispatcher-chat/dispatcherChatUtils";
import { cleanupDispatcherSession, subscribeDispatcherMessages } from "./dispatcherSessionStore";
import { ChatShell } from "./chat/chat-shell";
import type { ComposerMode } from "./chat/prompt-input";
import type { DispatcherChatHandle } from "./dispatcher-chat/useDispatcherActions";
import { PythonRunDrawer } from "./dispatcher-chat/PythonRunDrawer";
import { Button } from "./ui/button";
import { Badge } from "./ui/badge";

/**
 * ChatPageV2 — the integration adapter between the new ChatShell UI and the
 * existing streaming pipeline.
 *
 * It owns:
 *   - message history (initial invoke + subscribeDispatcherMessages pub/sub,
 *     merged with mergeDispatcherMessages)
 *   - the composer state (input, attached images)
 *   - the useDispatcherActions hook (Tauri Channel streaming, reused verbatim)
 *   - stop / resume invokes
 *
 * Everything is passed down to <ChatShell /> as plain props. The new UI never
 * touches the streaming channel directly — the existing, battle-tested hook
 * does, unchanged.
 *
 * This is the single chat surface used by HomeChatPage and the ProjectPage
 * embedded chat pane.
 */
export interface ChatPageV2Props {
  sessionId?: string | null;
  onSessionChange?: (sessionId: string | null) => void;
  conversationKind?: "project" | "chat";
  projectPath?: string;
  mcpStatus?: ProjectMcpStatus | null;
  mcpChecking?: boolean;
  subProcesses?: SubProcess[];
  onOpenSettings: () => void;
  onDispatchApproved?: (
    dispatchId: string,
    agent: AgentType,
    description: string,
    taskPrompt: string,
    permissionMode: string,
    sessionId: string,
  ) => void;
  onDispatchRejected?: (dispatchId: string) => void;
  onDispatchContinue?: (agent: AgentType, text: string, sessionId: string) => void;
  onDispatchExit?: (agent: AgentType, reason: string, sessionId: string) => void;
  onStopActiveRun?: (sessionId: string) => Promise<void>;
  onResumeStoppedRun?: (sessionId: string) => Promise<void>;
  onOpenMcpStatus?: () => void;
  onClosePanel?: () => void;
  embedded?: boolean;
}

export const ChatPageV2 = forwardRef<DispatcherChatHandle, ChatPageV2Props>(
  function ChatPageV2(
    {
      sessionId,
      onSessionChange,
      conversationKind = "chat",
      projectPath = "",
      mcpStatus = null,
      mcpChecking = false,
      subProcesses = [],
      onOpenSettings,
      onDispatchApproved,
      onDispatchRejected,
      onDispatchContinue,
      onDispatchExit,
      onStopActiveRun,
      onResumeStoppedRun,
      onOpenMcpStatus,
      onClosePanel,
      embedded = false,
    },
    ref,
  ) {
  const [uncontrolledSessionId, setUncontrolledSessionId] = useState<string | null>(null);
  const activeSessionId = sessionId !== undefined ? sessionId : uncontrolledSessionId;
  const setActiveSessionId = useCallback(
    (nextSessionId: string | null) => {
      if (sessionId === undefined) {
        setUncontrolledSessionId(nextSessionId);
      }
      onSessionChange?.(nextSessionId);
    },
    [onSessionChange, sessionId],
  );
  const [input, setInput] = useState("");
  const [attachedImages, setAttachedImages] = useState<ImageSegment[]>([]);
  const [messages, setMessages] = useState<DispatcherMessage[]>([]);
  const [search, setSearch] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");
  const [autoApprove, setAutoApprove] = useState(false);
  const [isStopping, setIsStopping] = useState(false);
  const [pythonDrawerOpen, setPythonDrawerOpen] = useState(false);
  const [pythonRunTarget, setPythonRunTarget] = useState<PythonCodeRunTarget | null>(null);
  const [pythonRunRecords, setPythonRunRecords] = useState<Record<string, PythonCodeRunRecord>>(
    {},
  );

  const isPlainChat = conversationKind === "chat";
  useChatSessionUpdates(isPlainChat && !embedded);
  const sessionsQuery = useChatSessionsQuery(undefined, isPlainChat && !embedded);
  const categoriesQuery = useChatCategoriesQuery(isPlainChat && !embedded);
  const createCategory = useCreateChatCategory();
  const { mutateAsync: createChatSession } = useCreateChatSession();
  const updateCategory = useUpdateChatCategory();
  const deleteCategory = useDeleteChatCategory();
  const { mutateAsync: deleteChatSession } = useDeleteChatSession();
  const setSessionCategory = useSetChatSessionCategory();
  const sessionSearchQuery = useSessionSearchQuery({
    query: debouncedSearch,
    kind: "chat",
    enabled: isPlainChat && !embedded,
  });

  // ── Streaming pipeline (reused unchanged) ───────────────────────────────
  const { liveState, updateLiveSessionState } = useLiveSessionState(activeSessionId ?? "");
  const { refresh: refreshSessionTokenUsage } = useDispatcherSessionTokenUsage(
    activeSessionId ?? "",
  );

  const currentSessionIdRef = useRef<string | null>(activeSessionId);
  currentSessionIdRef.current = activeSessionId;
  const shouldStickToBottomRef = useRef(true);
  const autoApproveRef = useRef(autoApprove);
  autoApproveRef.current = autoApprove;
  const onDispatchApprovedRef = useRef(onDispatchApproved);
  onDispatchApprovedRef.current = onDispatchApproved;
  const onDispatchContinueRef = useRef(onDispatchContinue);
  onDispatchContinueRef.current = onDispatchContinue;
  const onDispatchExitRef = useRef(onDispatchExit);
  onDispatchExitRef.current = onDispatchExit;

  const scrollMessageListToBottom = useCallback(() => {
    // The new MessageList owns its own scroll via useAutoScroll; the existing
    // pipeline calls this on send. We delegate by dispatching a custom event
    // the anchor could listen to — but since useAutoScroll already follows
    // new content while pinned, a no-op here is safe. Kept for API compat.
    if (shouldStickToBottomRef.current) {
      const el = document.querySelector('[role="log"]');
      if (el) el.scrollTop = el.scrollHeight;
    }
  }, []);

  const actions = useDispatcherActions({
    sessionId: activeSessionId ?? "",
    projectPath,
    isPlainChat,
    updateLiveSessionState,
    scrollMessageListToBottom,
    currentSessionIdRef: currentSessionIdRef as React.RefObject<string>,
    refreshSessionTokenUsage,
    autoApproveRef,
    onDispatchApprovedRef,
    onDispatchContinueRef,
    onDispatchExitRef,
    shouldStickToBottomRef,
    setInput,
    setAttachedImages,
  });

  useImperativeHandle(
    ref,
    () => ({
      continueWithResult: actions.continueWithResult,
    }),
    [actions.continueWithResult],
  );

  useEffect(() => {
    invoke<AhaSettingsV2>("aha_get_settings_v2")
      .then((settings) => setAutoApprove(settings.autoApproveDispatch))
      .catch(console.error);
  }, []);

  useEffect(() => {
    const timer = window.setTimeout(() => setDebouncedSearch(search), 260);
    return () => window.clearTimeout(timer);
  }, [search]);

  useEffect(() => {
    if (!activeSessionId) {
      setPythonRunTarget(null);
      setPythonRunRecords({});
      return;
    }

    let cancelled = false;
    invoke<PythonCodeRunRecord[]>("python_runner_list_results", { workspaceId: activeSessionId })
      .then((records) => {
        if (!cancelled) {
          setPythonRunRecords(indexPythonRuns(records));
        }
      })
      .catch(console.error);

    return () => {
      cancelled = true;
    };
  }, [activeSessionId]);

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

      if (payload.event !== "output") return;
      setPythonRunRecords((prev) => {
        const entry = Object.entries(prev).find(([, existing]) => existing.runId === payload.runId);
        if (!entry) return prev;
        const [key, existing] = entry;
        return {
          ...prev,
          [key]: {
            ...existing,
            stdout: existing.stdout + (payload.data.stdout ?? ""),
            stderr: existing.stderr + (payload.data.stderr ?? ""),
          },
        };
      });
    });

    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);

  // ── Message history: initial load + live pub/sub ────────────────────────
  useEffect(() => {
    if (!activeSessionId) {
      setMessages([]);
      return;
    }
    let cancelled = false;
    setMessages([]);
    void (async () => {
      try {
        const initial = await invoke<DispatcherMessage[]>(
          "dispatcher_list_messages",
          { workspaceId: activeSessionId },
        );
        // Backend serializes segments as `segmentsJson` (string), not `segments`.
        // Normalize here so every downstream consumer (UserMessage, buildItems,
        // ...) sees a real array — same hydration the live pub/sub path uses.
        if (!cancelled) setMessages(mergeDispatcherMessages([], initial));
      } catch (err) {
        console.error("加载会话消息失败:", err);
      }
    })();

    const unsubscribe = subscribeDispatcherMessages(activeSessionId, (incoming) => {
      setMessages((prev) => mergeDispatcherMessages(prev, incoming));
    });

    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, [activeSessionId]);

  // ── Composer mode (send / stop / resume) ────────────────────────────────
  const sessionSubProcesses = useMemo(
    () =>
      activeSessionId
        ? subProcesses.filter((subProcess) => subProcess.sessionId === activeSessionId)
        : [],
    [activeSessionId, subProcesses],
  );
  const hasRunningSubProcess =
    !isPlainChat && sessionSubProcesses.some((subProcess) => subProcess.status === "running");
  const hasStoppedSubProcess =
    !isPlainChat && sessionSubProcesses.some((subProcess) => subProcess.status === "stopped");
  const isRunning = Boolean(
    liveState.hasPendingRun || liveState.isLoading || hasRunningSubProcess || isStopping,
  );
  const composerMode: ComposerMode = isRunning ? "stop" : hasStoppedSubProcess ? "resume" : "send";

  // 聊天模式下，若还没有活跃会话，发送时懒创建一个。
  // 重构后 Sidebar/ChatPageV2 不再像旧 ChatSessionSidebar 那样在初始化时
  // 自动建会话，因此首次进入或点「新建对话」后 activeSessionId 为 null，
  // 这里补回懒创建，避免发送按钮被静默拦截（按钮看起来可点却无反应）。
  // pendingSessionIdRef 防止连点发送时重复创建会话。
  const pendingSessionIdRef = useRef<Promise<string | null> | null>(null);
  const ensurePlainChatSession = useCallback(async (): Promise<string | null> => {
    if (activeSessionId) return activeSessionId;
    if (!isPlainChat) return null;
    if (pendingSessionIdRef.current) return pendingSessionIdRef.current;
    const pending = (async () => {
      try {
        const session = await createChatSession({
          title: "新对话",
          category: "tech",
        });
        setActiveSessionId(session.id);
        return session.id;
      } catch (err) {
        console.error("创建聊天会话失败:", err);
        return null;
      } finally {
        pendingSessionIdRef.current = null;
      }
    })();
    pendingSessionIdRef.current = pending;
    return pending;
  }, [activeSessionId, createChatSession, isPlainChat, setActiveSessionId]);

  const handleSend = useCallback(() => {
    const text = input.trim();
    if (!text && attachedImages.length === 0) return;
    void (async () => {
      // 项目模式总会话 id 非空（ProjectPage 仅在 activeSessionId 存在时渲染本组件）；
      // 聊天模式可能为 null，懒创建后再发送。
      const targetSessionId = activeSessionId ?? (await ensurePlainChatSession());
      if (!targetSessionId) return;
      void actions.sendUserMessage(text, attachedImages, targetSessionId);
    })();
  }, [actions, activeSessionId, attachedImages, ensurePlainChatSession, input]);

  const handleStop = useCallback(async () => {
    if (!activeSessionId || isStopping) return;
    setIsStopping(true);
    try {
      await Promise.all([
        invoke("dispatcher_stop_run", { workspaceId: activeSessionId }).catch(console.error),
        onStopActiveRun?.(activeSessionId) ?? Promise.resolve(),
      ]);
    } catch (err) {
      console.error("停止生成失败:", err);
    } finally {
      setIsStopping(false);
    }
  }, [activeSessionId, isStopping, onStopActiveRun]);

  const handleResume = useCallback(async () => {
    if (!activeSessionId) return;
    await onResumeStoppedRun?.(activeSessionId);
  }, [activeSessionId, onResumeStoppedRun]);

  const handleRegenerate = useCallback(() => {
    if (!activeSessionId || isRunning) return;

    const lastUserMessage = [...messages].reverse().find((message) => message.role === "user");
    if (!lastUserMessage) {
      console.error("重新生成失败：当前会话没有可重发的用户消息");
      return;
    }

    const { text, images } = getUserMessagePayload(lastUserMessage);
    if (!text && images.length === 0) {
      console.error("重新生成失败：最后一条用户消息没有可重发内容");
      return;
    }

    void actions.sendUserMessage(text, images, activeSessionId);
  }, [actions, activeSessionId, isRunning, messages]);

  const handleNewConversation = useCallback(() => {
    if (!embedded) {
      setActiveSessionId(null);
    }
    setInput("");
    setAttachedImages([]);
    setMessages([]);
  }, [embedded, setActiveSessionId]);

  // 在指定分类下新建会话：真正落库一个空会话并激活它。
  // 侧边栏每个分类行内的 + 按钮走这里，可指定分类；
  // 顶部「新建对话」按钮已移除，因为它无法指定分类。
  const handleNewSessionInCategory = useCallback(
    async (categoryId: string) => {
      if (!isPlainChat || embedded) return;
      try {
        const session = await createChatSession({
          title: "新对话",
          category: categoryId,
        });
        setActiveSessionId(session.id);
        setInput("");
        setAttachedImages([]);
        setMessages([]);
      } catch (err) {
        console.error("在分类下创建会话失败:", err);
      }
    },
    [createChatSession, embedded, isPlainChat, setActiveSessionId],
  );

  const handleDeleteSession = useCallback(
    async (sessionIdToDelete: string) => {
      if (!isPlainChat || embedded) return;
      const confirmed = await confirm("确定永久删除这个会话吗？相关消息和文件也会一并删除。", {
        title: "删除会话",
        kind: "warning",
      });
      if (!confirmed) return;

      try {
        await deleteChatSession(sessionIdToDelete);
        cleanupDispatcherSession(sessionIdToDelete);
        if (sessionIdToDelete === activeSessionId) {
          const nextSession = (sessionsQuery.data ?? []).find(
            (session) => session.id !== sessionIdToDelete,
          );
          setActiveSessionId(nextSession?.id ?? null);
          setInput("");
          setAttachedImages([]);
          setMessages([]);
        }
      } catch (err) {
        console.error("删除聊天会话失败:", err);
      }
    },
    [
      activeSessionId,
      deleteChatSession,
      embedded,
      isPlainChat,
      sessionsQuery.data,
      setActiveSessionId,
    ],
  );

  const handleActiveSessionChange = useCallback((id: string | null) => {
    setActiveSessionId(id);
    setInput("");
    setAttachedImages([]);
  }, [setActiveSessionId]);

  const handleClearMessages = useCallback(async () => {
    if (!activeSessionId) return;
    await invoke("dispatcher_clear_messages", { workspaceId: activeSessionId });
    setMessages([]);
  }, [activeSessionId]);

  const handleToggleAutoApprove = useCallback(async () => {
    const next = !autoApprove;
    setAutoApprove(next);
    try {
      const settings = await invoke<AhaSettingsV2>("aha_get_settings_v2");
      const saved = await invoke<AhaSettingsV2>("aha_save_settings_v2", {
        settings: { ...settings, autoApproveDispatch: next },
      });
      setAutoApprove(saved.autoApproveDispatch);
    } catch (error) {
      setAutoApprove(!next);
      console.error("更新 Aha 自动批准设置失败:", error);
    }
  }, [autoApprove]);

  const handleCreateCategory = useCallback(
    (name: string, config?: { systemPrompt?: string; allowedTools?: string[] }) => {
      if (!isPlainChat || embedded) return;
      createCategory.mutate({
        name,
        systemPrompt: config?.systemPrompt,
        allowedTools: config?.allowedTools,
      });
    },
    [createCategory, embedded, isPlainChat],
  );

  const handleRenameCategory = useCallback(
    (categoryId: string, name: string) => {
      if (!isPlainChat || embedded) return;
      updateCategory.mutate({ categoryId, name });
    },
    [embedded, isPlainChat, updateCategory],
  );

  const handleDeleteCategory = useCallback(
    (categoryId: string) => {
      if (!isPlainChat || embedded) return;
      deleteCategory.mutate(categoryId);
    },
    [deleteCategory, embedded, isPlainChat],
  );

  const handleMoveSessionToCategory = useCallback(
    (workspaceId: string, categoryId: string) => {
      if (!isPlainChat || embedded) return;
      setSessionCategory.mutate({ workspaceId, categoryId });
      if (workspaceId === activeSessionId) {
        onSessionChange?.(workspaceId);
      }
    },
    [activeSessionId, embedded, isPlainChat, onSessionChange, setSessionCategory],
  );

  const handleApproveDispatch = useCallback(
    (dispatchId: string, taskPrompt: string) => {
      if (!activeSessionId) return;
      const currentPendingDispatch = liveState.pendingDispatches.find(
        (dispatch) => dispatch.dispatchId === dispatchId,
      );
      if (!currentPendingDispatch) return;
      updateLiveSessionState(activeSessionId, (state) => ({
        ...state,
        pendingDispatches: state.pendingDispatches.filter(
          (dispatch) => dispatch.dispatchId !== dispatchId,
        ),
      }));
      onDispatchApproved?.(
        dispatchId,
        currentPendingDispatch.agent,
        currentPendingDispatch.description,
        taskPrompt,
        currentPendingDispatch.permissionMode,
        activeSessionId,
      );
    },
    [activeSessionId, liveState.pendingDispatches, onDispatchApproved, updateLiveSessionState],
  );

  const handleRejectDispatch = useCallback(
    (dispatchId: string) => {
      if (!activeSessionId) return;
      updateLiveSessionState(activeSessionId, (state) => ({
        ...state,
        pendingDispatches: state.pendingDispatches.filter(
          (dispatch) => dispatch.dispatchId !== dispatchId,
        ),
      }));
      onDispatchRejected?.(dispatchId);
    },
    [activeSessionId, onDispatchRejected, updateLiveSessionState],
  );

  const handleRunPython = useCallback(
    async (target: PythonCodeRunTarget) => {
      if (!activeSessionId) return;
      setPythonRunTarget(target);
      setPythonDrawerOpen(true);
      try {
        const started = await invoke<PythonCodeRunRecord>("python_runner_start", {
          workspaceId: activeSessionId,
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
    },
    [activeSessionId],
  );

  const handleStopPythonRun = useCallback(async (runId: string) => {
    try {
      await invoke("python_runner_stop", { runId });
    } catch (error) {
      console.error("停止 Python 执行失败:", error);
    }
  }, []);

  const handleClearPythonRun = useCallback(
    async (target: PythonCodeRunTarget) => {
      if (!activeSessionId) return;
      try {
        await invoke("python_runner_clear_result", {
          workspaceId: activeSessionId,
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
    },
    [activeSessionId],
  );

  const projectHeader = !isPlainChat ? (
    <ProjectChatHeader
      isLoading={liveState.isLoading || liveState.hasPendingRun}
      hasMessages={messages.length > 0}
      autoApprove={autoApprove}
      mcpStatus={mcpStatus}
      mcpChecking={mcpChecking}
      onToggleAutoApprove={handleToggleAutoApprove}
      onOpenMcpStatus={onOpenMcpStatus}
      onClearMessages={handleClearMessages}
      onOpenSettings={onOpenSettings}
      onClosePanel={onClosePanel}
    />
  ) : undefined;

  const selectedPythonRun = pythonRunTarget
    ? pythonRunRecords[pythonRunKey(pythonRunTarget.messageId, pythonRunTarget.codeHash)] ?? null
    : null;
  const selectedPythonRunning = selectedPythonRun?.status === "running";
  const trimmedSearch = debouncedSearch.trim();
  const displayedSessions =
    trimmedSearch.length > 0
      ? (sessionSearchQuery.data ?? []).map((result) => ({
          id: result.sessionId,
          title: result.sessionTitle,
          category: result.category,
          createdAt: result.updatedAt,
          updatedAt: result.updatedAt,
          keywords: result.keywords,
        }))
      : (sessionsQuery.data ?? []);
  const sessionsLoading =
    trimmedSearch.length > 0
      ? sessionSearchQuery.isLoading
      : sessionsQuery.isLoading || categoriesQuery.isLoading;
  const sessionsError =
    trimmedSearch.length > 0 && sessionSearchQuery.error
      ? String(sessionSearchQuery.error)
      : undefined;

  return (
    <div className="flex h-full w-full min-w-0 overflow-hidden">
      <div className="min-w-0 flex-1">
        <ChatShell
          sessionId={activeSessionId}
          messages={messages}
          sessions={displayedSessions}
          categories={categoriesQuery.data ?? []}
          sessionsLoading={sessionsLoading}
          sessionsError={sessionsError}
          searchActive={trimmedSearch.length > 0}
          onActiveSessionChange={handleActiveSessionChange}
          onNewConversation={handleNewConversation}
          onNewSessionInCategory={
            isPlainChat && !embedded ? handleNewSessionInCategory : undefined
          }
          onDeleteSession={isPlainChat && !embedded ? handleDeleteSession : undefined}
          searchValue={search}
          onSearchChange={setSearch}
          onOpenSettings={onOpenSettings}
          onCreateCategory={isPlainChat && !embedded ? handleCreateCategory : undefined}
          onRenameCategory={isPlainChat && !embedded ? handleRenameCategory : undefined}
          onDeleteCategory={isPlainChat && !embedded ? handleDeleteCategory : undefined}
          onMoveSessionToCategory={
            isPlainChat && !embedded ? handleMoveSessionToCategory : undefined
          }
          input={input}
          onInputChange={setInput}
          composerMode={composerMode}
          onSend={handleSend}
          onStop={handleStop}
          onResume={handleResume}
          onRegenerate={isPlainChat ? handleRegenerate : undefined}
          onApproveDispatch={handleApproveDispatch}
          onRejectDispatch={handleRejectDispatch}
          pythonRunRecords={pythonRunRecords}
          onRunPython={handleRunPython}
          embedded={embedded}
          projectHeader={projectHeader}
        />
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
});

function getUserMessagePayload(message: DispatcherMessage): {
  text: string;
  images: ImageSegment[];
} {
  const segments = message.segments ?? [];
  const text = segments
    .filter(isTextSegment)
    .map((segment) => segment.text)
    .join("\n\n")
    .trim() || message.content.trim();
  const images = segments.filter(isImageSegment);
  return { text, images };
}

function isTextSegment(
  segment: AnyContentSegment,
): segment is Extract<AnyContentSegment, { type: "text" }> {
  return segment.type === "text";
}

function isImageSegment(segment: AnyContentSegment): segment is ImageSegment {
  return segment.type === "image";
}

function pythonRunKey(messageId: string, codeHash: string) {
  return `${messageId}:${codeHash}`;
}

function indexPythonRuns(records: PythonCodeRunRecord[]) {
  return records.reduce<Record<string, PythonCodeRunRecord>>((acc, record) => {
    acc[pythonRunKey(record.messageId, record.codeHash)] = record;
    return acc;
  }, {});
}

function ProjectChatHeader({
  isLoading,
  hasMessages,
  autoApprove,
  mcpStatus,
  mcpChecking,
  onToggleAutoApprove,
  onOpenMcpStatus,
  onClearMessages,
  onOpenSettings,
  onClosePanel,
}: {
  isLoading: boolean;
  hasMessages: boolean;
  autoApprove: boolean;
  mcpStatus: ProjectMcpStatus | null;
  mcpChecking: boolean;
  onToggleAutoApprove: () => void;
  onOpenMcpStatus?: () => void;
  onClearMessages: () => void;
  onOpenSettings: () => void;
  onClosePanel?: () => void;
}) {
  const mcpIndicator = getMcpIndicatorState(mcpStatus, mcpChecking);

  return (
    <div className="flex min-h-12 items-center gap-2 px-4">
      <div className="flex min-w-0 flex-1 items-center gap-2">
        <span className="text-sm font-medium">调度智能体</span>
        {isLoading && <span className="h-1.5 w-1.5 rounded-full bg-primary animate-pulse" />}
      </div>
      <Button
        variant={autoApprove ? "secondary" : "outline"}
        size="sm"
        onClick={onToggleAutoApprove}
      >
        免确认 {autoApprove ? "开" : "关"}
      </Button>
      <Button variant="outline" size="sm" onClick={onOpenMcpStatus}>
        <span
          className="h-2 w-2 rounded-full"
          style={{ background: mcpIndicator.color }}
        />
        MCP
      </Button>
      {hasMessages && (
        <Button variant="ghost" size="sm" onClick={onClearMessages}>
          清空
        </Button>
      )}
      <Button variant="ghost" size="sm" onClick={onOpenSettings}>
        设置
      </Button>
      {onClosePanel && (
        <Button variant="ghost" size="icon-sm" aria-label="关闭会话面板" onClick={onClosePanel}>
          ×
        </Button>
      )}
      <Badge variant="outline" className="hidden text-[10px] sm:inline-flex">
        {mcpIndicator.label}
      </Badge>
    </div>
  );
}

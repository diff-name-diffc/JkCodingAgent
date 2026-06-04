import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
  AgentType,
  ChecklistPlanState,
  DispatcherMode,
  DispatcherSessionRuntimeState,
  DispatcherSettings,
  ImageSegment,
  PlanInteraction,
} from "../../types";
import type { DispatcherChatHandle } from "./useDispatcherActions";
import type { LiveSessionUpdater } from "./useLiveSessionState";
import {
  toErrorMessage,
  buildPlanQuestionAnswer,
  buildPlanImplementationPrompt,
  isMessageListNearBottom,
} from "./dispatcherChatUtils";

export interface UseDispatcherHandlersOptions {
  sessionId: string;
  // Local state values
  input: string;
  attachedImages: ImageSegment[];
  isStopping: boolean;
  autoApprove: boolean;
  mode: DispatcherMode;
  planInteraction: PlanInteraction | null;
  // Refs
  inputRef: React.RefObject<HTMLTextAreaElement | null>;
  inputComposingRef: React.RefObject<boolean>;
  shouldStickToBottomRef: React.RefObject<boolean>;
  // Derived
  composerMode: "send" | "stop" | "resume";
  isLoading: boolean;
  currentPendingDispatch: { dispatchId: string; agent: AgentType; description: string; taskPrompt: string; permissionMode: string } | null;
  // Callbacks from other hooks
  actions: {
    sendUserMessage: (
      rawText: string,
      images?: ImageSegment[],
      targetSessionId?: string,
      targetMode?: DispatcherMode,
    ) => Promise<void>;
    continueWithResult: DispatcherChatHandle["continueWithResult"];
    applyRuntimeState: DispatcherChatHandle["applyRuntimeState"];
  };
  updateLiveSessionState: LiveSessionUpdater;
  voiceInput: {
    isRecording: boolean;
    stopRecording: () => Promise<void>;
    toggleRecording: () => void;
    transcript: string;
    error: string | null;
    clearError: () => void;
  };
  resetSessionTokenUsage: () => void;
  // State setters
  setMessages: React.Dispatch<React.SetStateAction<import("../../types").DispatcherMessage[]>>;
  setAttachedImages: React.Dispatch<React.SetStateAction<ImageSegment[]>>;
  setIsStopping: React.Dispatch<React.SetStateAction<boolean>>;
  setAutoApprove: React.Dispatch<React.SetStateAction<boolean>>;
  setMode: React.Dispatch<React.SetStateAction<DispatcherMode>>;
  setChecklist: React.Dispatch<React.SetStateAction<ChecklistPlanState | null>>;
  setPlanInteraction: React.Dispatch<React.SetStateAction<PlanInteraction | null>>;
  setActivePlanPath: React.Dispatch<React.SetStateAction<string | null>>;
  setImplementingPlan: React.Dispatch<React.SetStateAction<boolean>>;
  setThinkingEnabled: React.Dispatch<React.SetStateAction<boolean>>;
  // Props callbacks
  onDispatchApproved?: (dispatchId: string, agent: AgentType, description: string, taskPrompt: string, permissionMode: string, sessionId: string) => void;
  onDispatchRejected?: (dispatchId: string) => void;
  onStopActiveRun?: (sessionId: string) => Promise<void>;
  onResumeStoppedRun?: (sessionId: string) => Promise<void>;
}

export interface UseDispatcherHandlersResult {
  handlePaste: (e: React.ClipboardEvent) => void;
  handleDrop: (e: React.DragEvent) => void;
  handleDragOver: (e: React.DragEvent) => void;
  handleRemoveImage: (index: number) => void;
  handleSend: () => Promise<void>;
  onStop: () => Promise<void>;
  onResume: () => Promise<void>;
  handleKeyDown: (e: React.KeyboardEvent) => void;
  handleApproveDispatch: (dispatchId: string, taskPrompt: string) => void;
  handleRejectDispatch: (dispatchId: string) => void;
  handleToggleAutoApprove: () => Promise<void>;
  handleModeChange: (nextMode: DispatcherMode) => Promise<void>;
  handleModeToggle: (nextMode: DispatcherMode) => void;
  handleToggleThinking: () => void;
  handleAnswerPlanQuestion: (answer: string) => Promise<void>;
  handleImplementPlan: (interaction: Extract<PlanInteraction, { kind: "ready" }>) => Promise<void>;
  handleImplementPlanWithClearedContext: (interaction: Extract<PlanInteraction, { kind: "ready" }>) => Promise<void>;
  handleStayInPlanMode: () => void;
  handleClearHistory: () => Promise<void>;
  handleMessageListScroll: (event: React.UIEvent<HTMLDivElement>) => void;
}

export function useDispatcherHandlers({
  sessionId,
  input,
  attachedImages,
  isStopping,
  autoApprove,
  mode,
  planInteraction,
  inputRef,
  inputComposingRef,
  shouldStickToBottomRef,
  composerMode,
  isLoading,
  currentPendingDispatch,
  actions,
  updateLiveSessionState,
  voiceInput,
  resetSessionTokenUsage,
  setMessages,
  setAttachedImages,
  setIsStopping,
  setAutoApprove,
  setMode,
  setChecklist,
  setPlanInteraction,
  setActivePlanPath,
  setImplementingPlan,
  setThinkingEnabled,
  onDispatchApproved,
  onDispatchRejected,
  onStopActiveRun,
  onResumeStoppedRun,
}: UseDispatcherHandlersOptions): UseDispatcherHandlersResult {

  // ── Image paste ──────────────────────────────────────────────
  const handlePaste = useCallback(
    (e: React.ClipboardEvent) => {
      const items = e.clipboardData?.items;
      if (!items) return;
      for (let i = 0; i < items.length; i++) {
        if (items[i].type.indexOf("image") !== -1) {
          const blob = items[i].getAsFile();
          if (blob) {
            const reader = new FileReader();
            reader.onload = async (event) => {
              const base64 = (event.target?.result as string).split(",")[1];
              try {
                const result = await invoke<{ imageId: string; path: string; mimeType: string }>("save_chat_image", {
                  sessionId,
                  sessionTitle: "",
                  imageDataBase64: base64,
                  mimeType: blob.type,
                });
                setAttachedImages((prev) => [
                  ...prev,
                  {
                    id: crypto.randomUUID(),
                    type: "image",
                    imageId: result.imageId,
                    path: result.path,
                    source: "user_paste",
                    mimeType: result.mimeType,
                  } as ImageSegment,
                ]);
              } catch (err) {
                console.error("保存图片失败:", err);
              }
            };
            reader.readAsDataURL(blob);
          }
        }
      }
    },
    [sessionId, setAttachedImages],
  );

  // ── Image drag-and-drop ───────────────────────────────────────
  const handleDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.dataTransfer.dropEffect = "copy";
  }, []);

  const handleDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault();
      const files = e.dataTransfer.files;
      for (let i = 0; i < files.length; i++) {
        const file = files[i];
        if (!file.type.startsWith("image/")) continue;
        const reader = new FileReader();
        reader.onload = async (event) => {
          const base64 = (event.target?.result as string).split(",")[1];
          try {
              const result = await invoke<{ imageId: string; path: string; mimeType: string }>("save_chat_image", {
              sessionId,
              sessionTitle: "",
              imageDataBase64: base64,
              mimeType: file.type,
            });
            setAttachedImages((prev) => [
              ...prev,
              {
                id: crypto.randomUUID(),
                type: "image",
                imageId: result.imageId,
                path: result.path,
                source: "user_paste",
                mimeType: result.mimeType,
              } as ImageSegment,
            ]);
          } catch (err) {
            console.error("保存拖拽图片失败:", err);
          }
        };
        reader.readAsDataURL(file);
      }
    },
    [sessionId, setAttachedImages],
  );

  const handleRemoveImage = useCallback((index: number) => {
    setAttachedImages((prev) => prev.filter((_, i) => i !== index));
  }, [setAttachedImages]);

  // ── Core action handlers ────────────────────────────────────
  const handleSend = useCallback(async () => {
    const text = input.trim();
    if ((!text && attachedImages.length === 0) || isLoading || isStopping) return;
    try {
      await actions.sendUserMessage(text, attachedImages, sessionId);
    } finally {
      if (voiceInput.isRecording) {
        await voiceInput.stopRecording();
      }
    }
  }, [input, attachedImages, isLoading, isStopping, actions, sessionId, voiceInput]);

  const onStop = useCallback(async () => {
    if (isStopping) return;
    setIsStopping(true);
    try {
      if (voiceInput.isRecording) await voiceInput.stopRecording();
      await Promise.all([
        invoke("dispatcher_stop_run", { workspaceId: sessionId }).catch(console.error),
        onStopActiveRun?.(sessionId) ?? Promise.resolve(),
      ]);
    } finally {
      setIsStopping(false);
    }
  }, [isStopping, onStopActiveRun, sessionId, voiceInput, setIsStopping]);

  const onResume = useCallback(async () => {
    await onResumeStoppedRun?.(sessionId);
  }, [onResumeStoppedRun, sessionId]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (composerMode === "stop") return;
      if (inputComposingRef.current) return;
      // IME composing check via native event
      if ("isComposing" in e && (e as unknown as { isComposing: boolean }).isComposing) return;
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        if (composerMode === "resume" && !input.trim()) {
          onResume();
          return;
        }
        handleSend();
      }
    },
    [composerMode, handleSend, input, onResume, inputComposingRef],
  );

  // ── Dispatch approval ───────────────────────────────────────
  const handleApproveDispatch = useCallback(
    (dispatchId: string, taskPrompt: string) => {
      const agent = currentPendingDispatch?.agent ?? "claude";
      const pm = currentPendingDispatch?.permissionMode ?? "full_access";
      const description = currentPendingDispatch?.description ?? "未命名子任务";
      updateLiveSessionState(sessionId, (state) => ({
        ...state,
        pendingDispatches: state.pendingDispatches.slice(1),
      }));
      onDispatchApproved?.(dispatchId, agent, description, taskPrompt, pm, sessionId);
    },
    [currentPendingDispatch, onDispatchApproved, sessionId, updateLiveSessionState],
  );

  const handleRejectDispatch = useCallback(
    (dispatchId: string) => {
      updateLiveSessionState(sessionId, (state) => ({
        ...state,
        pendingDispatches: state.pendingDispatches.slice(1),
      }));
      invoke<DispatcherSessionRuntimeState>("dispatcher_clear_checklist_dispatch", {
        sessionId,
        dispatchId,
      })
        .then((state) => setChecklist(state.checklist ?? null))
        .catch(console.error);
      onDispatchRejected?.(dispatchId);
    },
    [onDispatchRejected, sessionId, updateLiveSessionState, setChecklist],
  );

  // ── Auto-approve toggle ─────────────────────────────────────
  const handleToggleAutoApprove = useCallback(async () => {
    const next = !autoApprove;
    setAutoApprove(next);
    try {
      const saved = await invoke<DispatcherSettings>("dispatcher_set_auto_approve_dispatch", {
        autoApproveDispatch: next,
      });
      setAutoApprove(saved.autoApproveDispatch);
    } catch (err) {
      setAutoApprove(!next);
      console.error("dispatcher_set_auto_approve_dispatch 失败:", err);
    }
  }, [autoApprove, setAutoApprove]);

  // ── Mode management ─────────────────────────────────────────
  const handleModeChange = useCallback(
    async (nextMode: DispatcherMode) => {
      if (nextMode === mode) return;
      const previousMode = mode;
      setMode(nextMode);
      try {
        const state = await invoke<DispatcherSessionRuntimeState>("dispatcher_set_session_mode", {
          sessionId,
          mode: nextMode,
        });
        setMode(state.mode);
        setChecklist(state.checklist ?? null);
        setPlanInteraction(state.planInteraction ?? null);
        setActivePlanPath(state.activePlanPath ?? null);
      } catch (err) {
        setMode(previousMode);
        updateLiveSessionState(sessionId, (state) => ({
          ...state,
          runError: `切换模式失败：${toErrorMessage(err)}`,
        }));
      }
    },
    [mode, sessionId, updateLiveSessionState, setMode, setChecklist, setPlanInteraction, setActivePlanPath],
  );

  const handleModeToggle = useCallback(
    (nextMode: DispatcherMode) => {
      handleModeChange(mode === nextMode ? "default" : nextMode).catch(console.error);
    },
    [handleModeChange, mode],
  );

  const handleToggleThinking = useCallback(() => {
    setThinkingEnabled((v) => !v);
  }, [setThinkingEnabled]);

  // ── Plan interaction handlers ────────────────────────────────
  const handleAnswerPlanQuestion = useCallback(
    async (answer: string) => {
      if (planInteraction?.kind !== "question") return;
      const content = buildPlanQuestionAnswer(planInteraction, answer);
      setPlanInteraction(null);
      await actions.sendUserMessage(content, [], sessionId, "plan");
    },
    [planInteraction, actions, sessionId, setPlanInteraction],
  );

  const handleImplementPlan = useCallback(
    async (interaction: Extract<PlanInteraction, { kind: "ready" }>) => {
      setImplementingPlan(true);
      try {
        const content = buildPlanImplementationPrompt(interaction.planPath);
        await handleModeChange("default");
        setPlanInteraction(null);
        await actions.sendUserMessage(content, [], sessionId, "default");
      } catch (err) {
        updateLiveSessionState(sessionId, (state) => ({
          ...state,
          runError: `实施计划失败：${toErrorMessage(err)}`,
        }));
      } finally {
        setImplementingPlan(false);
      }
    },
    [actions, handleModeChange, sessionId, updateLiveSessionState, setPlanInteraction, setImplementingPlan],
  );

  const handleImplementPlanWithClearedContext = useCallback(
    async (interaction: Extract<PlanInteraction, { kind: "ready" }>) => {
      setImplementingPlan(true);
      try {
        const content = buildPlanImplementationPrompt(interaction.planPath);
        await handleModeChange("default");
        await invoke("dispatcher_clear_message_context", { workspaceId: sessionId });
        setPlanInteraction(null);
        setChecklist(null);
        await actions.sendUserMessage(content, [], sessionId, "default");
      } catch (err) {
        updateLiveSessionState(sessionId, (state) => ({
          ...state,
          runError: `清除上下文后实施失败：${toErrorMessage(err)}`,
        }));
      } finally {
        setImplementingPlan(false);
      }
    },
    [actions, handleModeChange, sessionId, updateLiveSessionState, setPlanInteraction, setChecklist, setImplementingPlan],
  );

  const handleStayInPlanMode = useCallback(() => {
    setPlanInteraction(null);
    setMode("plan");
    inputRef.current?.focus();
  }, [setPlanInteraction, setMode, inputRef]);

  // ── Misc handlers ───────────────────────────────────────────
  const handleClearHistory = useCallback(async () => {
    try {
      await invoke("dispatcher_clear_messages", { workspaceId: sessionId });
      setMessages([]);
      setChecklist(null);
      setPlanInteraction(null);
      setActivePlanPath(null);
      resetSessionTokenUsage();
    } catch (err) {
      console.error("清空消息失败:", err);
    }
  }, [resetSessionTokenUsage, sessionId, setMessages, setChecklist, setPlanInteraction, setActivePlanPath]);

  const handleMessageListScroll = useCallback((event: React.UIEvent<HTMLDivElement>) => {
    shouldStickToBottomRef.current = isMessageListNearBottom(event.currentTarget);
  }, [shouldStickToBottomRef]);

  return {
    handlePaste,
    handleDrop,
    handleDragOver,
    handleRemoveImage,
    handleSend,
    onStop,
    onResume,
    handleKeyDown,
    handleApproveDispatch,
    handleRejectDispatch,
    handleToggleAutoApprove,
    handleModeChange,
    handleModeToggle,
    handleToggleThinking,
    handleAnswerPlanQuestion,
    handleImplementPlan,
    handleImplementPlanWithClearedContext,
    handleStayInPlanMode,
    handleClearHistory,
    handleMessageListScroll,
  };
}

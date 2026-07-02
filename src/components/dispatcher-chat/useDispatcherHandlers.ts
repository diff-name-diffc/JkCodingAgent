import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
  AgentType,
  DispatcherSettings,
  ImageSegment,
} from "../../types";
import type { DispatcherChatHandle } from "./useDispatcherActions";
import type { LiveSessionUpdater } from "./useLiveSessionState";
import {
  isMessageListNearBottom,
} from "./dispatcherChatUtils";

export interface UseDispatcherHandlersOptions {
  sessionId: string;
  // Local state values
  input: string;
  attachedImages: ImageSegment[];
  isStopping: boolean;
  autoApprove: boolean;
  shouldStickToBottomRef: React.RefObject<boolean>;
  // Derived
  isLoading: boolean;
  currentPendingDispatch: { dispatchId: string; agent: AgentType; description: string; taskPrompt: string; permissionMode: string } | null;
  // Callbacks from other hooks
  actions: {
    sendUserMessage: (
      rawText: string,
      images?: ImageSegment[],
      targetSessionId?: string,
    ) => Promise<void>;
    continueWithResult: DispatcherChatHandle["continueWithResult"];
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
  handleApproveDispatch: (dispatchId: string, taskPrompt: string) => void;
  handleRejectDispatch: (dispatchId: string) => void;
  handleToggleAutoApprove: () => Promise<void>;
  handleClearHistory: () => Promise<void>;
  handleMessageListScroll: (event: React.UIEvent<HTMLDivElement>) => void;
}

export function useDispatcherHandlers({
  sessionId,
  input,
  attachedImages,
  isStopping,
  autoApprove,
  shouldStickToBottomRef,
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
      onDispatchRejected?.(dispatchId);
    },
    [onDispatchRejected, sessionId, updateLiveSessionState],
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

  // ── Misc handlers ───────────────────────────────────────────
  const handleClearHistory = useCallback(async () => {
    try {
      await invoke("dispatcher_clear_messages", { workspaceId: sessionId });
      setMessages([]);
      resetSessionTokenUsage();
    } catch (err) {
      console.error("清空消息失败:", err);
    }
  }, [resetSessionTokenUsage, sessionId, setMessages]);

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
    handleApproveDispatch,
    handleRejectDispatch,
    handleToggleAutoApprove,
    handleClearHistory,
    handleMessageListScroll,
  };
}

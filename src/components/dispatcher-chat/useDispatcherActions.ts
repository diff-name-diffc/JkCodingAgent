import { useRef, useCallback } from "react";
import { invoke, Channel } from "@tauri-apps/api/core";
import type { DispatcherAgentEvent, DispatcherAgentTurn, ImageSegment } from "../../types";
import {
  createIdleLiveSessionState,
  getDispatcherActiveRunId,
  nextDispatcherActiveRunId,
} from "../dispatcherSessionStore";
import type { LiveSessionUpdater } from "./useLiveSessionState";
import { toErrorMessage, createEmptyUsageStats } from "./dispatcherChatUtils";
import { createDispatcherEventChannel } from "./event-channel";

export interface UseDispatcherActionsOptions {
  sessionId: string;
  projectPath: string;
  isPlainChat: boolean;
  updateLiveSessionState: LiveSessionUpdater;
  scrollMessageListToBottom: () => void;
  currentSessionIdRef: React.RefObject<string>;
  refreshSessionTokenUsage: (targetSessionId?: string) => Promise<void>;
  shouldStickToBottomRef: React.RefObject<boolean>;
  setInput: (value: string) => void;
  setAttachedImages: React.Dispatch<React.SetStateAction<ImageSegment[]>>;
}

export interface UseDispatcherActionsResult {
  enqueueDispatcherRun: (
    targetSessionId: string,
    runner: (onEvent: Channel<DispatcherAgentEvent>) => Promise<void>,
  ) => Promise<void>;
  sendUserMessage: (
    rawText: string,
    images?: ImageSegment[],
    targetSessionId?: string,
  ) => Promise<void>;
}

export function useDispatcherActions({
  sessionId,
  projectPath,
  isPlainChat,
  updateLiveSessionState,
  scrollMessageListToBottom,
  currentSessionIdRef,
  refreshSessionTokenUsage,
  shouldStickToBottomRef,
  setInput,
  setAttachedImages,
}: UseDispatcherActionsOptions): UseDispatcherActionsResult {
  const runQueuesRef = useRef<Map<string, Promise<void>>>(new Map());

  const createEventChannel = useCallback(
    (targetSessionId: string, runId: number) =>
      createDispatcherEventChannel({
        targetSessionId,
        runId,
        updateLiveSessionState,
        refreshSessionTokenUsage,
      }),
    [refreshSessionTokenUsage, updateLiveSessionState],
  );

  const enqueueDispatcherRun = useCallback(
    async (
      targetSessionId: string,
      runner: (onEvent: Channel<DispatcherAgentEvent>) => Promise<void>,
    ) => {
      const previous = runQueuesRef.current.get(targetSessionId) ?? Promise.resolve();
      const queued = previous
        .catch(() => undefined)
        .then(async () => {
          const runId = nextDispatcherActiveRunId(targetSessionId);
          const now = Date.now();
          updateLiveSessionState(targetSessionId, () => ({
            ...createIdleLiveSessionState(),
            hasPendingRun: true,
            isLoading: true,
            activeUsageStats: createEmptyUsageStats(),
            activeUsageStatsReceivedAt: now,
            usageClockNow: now,
          }));

          const onEvent = createEventChannel(targetSessionId, runId);

          try {
            await runner(onEvent);
          } finally {
            if (getDispatcherActiveRunId(targetSessionId) === runId) {
              updateLiveSessionState(targetSessionId, (state) => ({
                ...state,
                hasPendingRun: false,
                isLoading: false,
                activeUsageStats: null,
              }));
            }
          }
        });

      runQueuesRef.current.set(targetSessionId, queued);

      try {
        await queued;
      } finally {
        if (runQueuesRef.current.get(targetSessionId) === queued) {
          runQueuesRef.current.delete(targetSessionId);
        }
      }
    },
    [createEventChannel, updateLiveSessionState],
  );

  const sendUserMessage = useCallback(
    async (rawText: string, images: ImageSegment[] = [], targetSessionId = sessionId) => {
      const text = rawText.trim();
      if (!text && images.length === 0) return;

      setInput("");
      setAttachedImages([]);
      if (currentSessionIdRef.current === targetSessionId) {
        shouldStickToBottomRef.current = true;
        window.requestAnimationFrame(() => scrollMessageListToBottom());
      }

      const segments: Array<{ type: string; [key: string]: unknown }> = [];
      for (const img of images) {
        segments.push({ ...img });
      }
      if (text) {
        segments.push({
          id: crypto.randomUUID(),
          type: "text",
          text,
        });
      }
      const segmentsJson = JSON.stringify(segments);

      try {
        await enqueueDispatcherRun(targetSessionId, async (onEvent) => {
          if (isPlainChat) {
            await invoke<DispatcherAgentTurn>("dispatcher_send_chat_agent_message", {
              workspaceId: targetSessionId,
              segmentsJson,
              onEvent,
            });
          } else {
            await invoke<DispatcherAgentTurn>("dispatcher_send_project_agent_message", {
              workspaceId: targetSessionId,
              projectPath,
              segmentsJson,
              onEvent,
            });
          }
        });
      } catch (err) {
        console.error("发送消息失败:", err);
        updateLiveSessionState(targetSessionId, (state) => ({
          ...state,
          runError: `${isPlainChat ? "聊天" : "调度智能体"}执行失败：${toErrorMessage(err)}`,
        }));
      }
    },
    [
      currentSessionIdRef,
      enqueueDispatcherRun,
      isPlainChat,
      projectPath,
      scrollMessageListToBottom,
      sessionId,
      setAttachedImages,
      setInput,
      shouldStickToBottomRef,
      updateLiveSessionState,
    ],
  );

  return {
    enqueueDispatcherRun,
    sendUserMessage,
  };
}

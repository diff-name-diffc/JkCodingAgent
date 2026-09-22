import { useRef, useCallback, useMemo } from "react";
import { invoke, Channel } from "@tauri-apps/api/core";
import type { DispatcherAgentEvent, DispatcherAgentTurn, ImageSegment } from "../../types";
import {
  clearDispatcherActiveRunId,
  createIdleLiveSessionState,
  getDispatcherActiveRunId,
  nextDispatcherActiveRunId,
  notifyDispatcherMessages,
} from "../dispatcherSessionStore";
import type { LiveSessionUpdater } from "./useLiveSessionState";
import { buildOptimisticUserMessage, toErrorMessage } from "./dispatcherChatUtils";
import { createDispatcherEventChannel, reconcileSessionMessages } from "./event-channel";

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
          updateLiveSessionState(targetSessionId, () => ({
            ...createIdleLiveSessionState(),
            hasPendingRun: true,
            isLoading: true,
            // 发送即占位：run 入口（设置读取、MCP 刷新、用户消息落库）可能
            // 耗时数秒，期间流式气泡以 placeholder 形式立即出现；后端
            // assistantStarted 事件会接续更新文案。
            assistantPlaceholder: "正在发送…",
          }));

          const onEvent = createEventChannel(targetSessionId, runId);

          try {
            await runner(onEvent);
          } finally {
            if (getDispatcherActiveRunId(targetSessionId) === runId) {
              // 兜底清槽：正常路径 finished/failed 事件已清除（此处 delete 幂等）；
              // 命令 reject 且无 failed 事件的路径（如 Agent 构建失败）在此
              // 清除，否则残留的活动 run 槽位会拦下后续消息对账。
              clearDispatcherActiveRunId(targetSessionId);
              updateLiveSessionState(targetSessionId, (state) => ({
                ...state,
                hasPendingRun: false,
                isLoading: false,
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

      // 乐观注入：不等后端 userMessage 事件往返，发送瞬间即渲染用户消息。
      // pending 标记由 mergeDispatcherMessages 在首批权威消息到达时替换
      // （run 失败则由对账批次丢弃），见 dispatcherChatUtils 的替换规则。
      notifyDispatcherMessages(targetSessionId, [
        buildOptimisticUserMessage(targetSessionId, text, images),
      ]);

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
          // Failed 事件若已带完整错误链，保留它；命令层 reject 且未发
          // failed 时（如 Agent 构建失败）才用 invoke 错误兜底。
          runError:
            state.runError ??
            `${isPlainChat ? "聊天" : "调度智能体"}执行失败：${toErrorMessage(err)}`,
        }));
        // 命令层 reject（如 Agent 构建失败）不一定伴随 failed 事件；对账
        // 兜底把已持久化消息刷进列表并清掉乐观 pending 消息。
        reconcileSessionMessages(targetSessionId);
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

  // UI-24a-1：返回对象 memo 化——此前每次渲染返回新字面量，消费方
  // useCallback(deps 含 actions) 连带换身份，击穿 MessageItem 的 React.memo。
  return useMemo(
    () => ({
      enqueueDispatcherRun,
      sendUserMessage,
    }),
    [enqueueDispatcherRun, sendUserMessage],
  );
}

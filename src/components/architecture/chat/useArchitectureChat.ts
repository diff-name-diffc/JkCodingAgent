/**
 * 架构助手聊天面板的数据钩子：会话懒创建、消息历史、事件流、双重感知
 * （截图 + 结构化快照）发送、串行执行队列与停止。
 *
 * 后端入口：`dispatcher_send_architecture_agent_message`（按面板选择的视觉
 * 模型库条目构建 Agent）；执行往返由 `useArchRunListener` 在画布侧承接。
 */

import { useCallback, useRef, useState } from "react";
import { invoke, type Channel } from "@tauri-apps/api/core";
import { exportToBlob } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import type {
  AnyContentSegment,
  ChatSession,
  DispatcherAgentEvent,
  DispatcherAgentTurn,
  DispatcherMessage,
} from "../../../types";
import { ARCH_DESIGN_CATEGORY } from "../../../types/architecture";
import {
  clearDispatcherActiveRunId,
  createIdleLiveSessionState,
  getDispatcherActiveRunId,
  getDispatcherSessionRunning,
  nextDispatcherActiveRunId,
  notifyDispatcherMessages,
} from "../../dispatcherSessionStore";
import {
  createDispatcherEventChannel,
  reconcileSessionMessages,
  type DispatcherEventChannelDeps,
} from "../../dispatcher-chat/event-channel";
import {
  buildOptimisticUserMessage,
  toErrorMessage,
} from "../../dispatcher-chat/dispatcherChatUtils";
import { useChatMessages } from "../../chat-page-v2/useChatMessages";
import {
  useDispatcherSessionRunning,
  useLiveSessionUpdater,
} from "../../dispatcher-chat/useLiveSessionState";
import { collectCanvasSnapshot } from "../canvas-snapshot";
import { blobToBase64, canvasExportOptions } from "../program/arch-executor";
import {
  loadArchChatPrefs,
  saveArchChatPrefs,
  type ArchitectureChatPrefs,
} from "./architecture-chat-prefs";

export interface UseArchitectureChatOptions {
  getCanvasApi: () => ExcalidrawImperativeAPI | null;
}

export interface UseArchitectureChatResult {
  sessionId: string | null;
  messages: DispatcherMessage[];
  isRunning: boolean;
  /** 最近一次发送失败的可见错误（覆盖 ensureSession 失败的无会话场景）。 */
  sendError: string | null;
  prefs: ArchitectureChatPrefs;
  updatePrefs: (patch: Partial<ArchitectureChatPrefs>) => void;
  /** 发送成功返回 true；失败时置 sendError 并返回 false（供调用方恢复输入）。 */
  send: (text: string) => Promise<boolean>;
  stop: () => Promise<void>;
  newConversation: () => void;
}

export function useArchitectureChat({
  getCanvasApi,
}: UseArchitectureChatOptions): UseArchitectureChatResult {
  const [prefs, setPrefs] = useState<ArchitectureChatPrefs>(loadArchChatPrefs);
  const [sendError, setSendError] = useState<string | null>(null);
  const prefsRef = useRef(prefs);
  prefsRef.current = prefs;
  const getCanvasApiRef = useRef(getCanvasApi);
  getCanvasApiRef.current = getCanvasApi;

  const sessionId = prefs.sessionId;
  const noopResetEditing = useCallback(() => {}, []);
  const { messages } = useChatMessages(sessionId, noopResetEditing);
  // 面板级只需要 run 边界布尔（token 流每帧通知由 MessageList 内部消化）。
  const isSessionRunning = useDispatcherSessionRunning(sessionId);
  // rAF 合帧更新器与主聊天共用（useDispatcherActions 同款），不再手写副本。
  const updateLiveSessionState = useLiveSessionUpdater();

  const updatePrefs = useCallback((patch: Partial<ArchitectureChatPrefs>) => {
    // 副作用（ref 写入 + localStorage 落盘）必须在 setState 更新器之外：
    // 更新器在 StrictMode 下双调、有并发更新时会推迟执行，prefsRef 会读到旧值。
    const next = { ...prefsRef.current, ...patch };
    prefsRef.current = next;
    saveArchChatPrefs(next);
    setPrefs(next);
  }, []);

  // ── 会话懒创建（并发防重：首个调用建会话，其余等同一 Promise）──
  const sessionPromiseRef = useRef<Promise<string> | null>(null);
  const ensureSession = useCallback(async (): Promise<string> => {
    const existing = prefsRef.current.sessionId;
    if (existing) return existing;
    if (sessionPromiseRef.current) return sessionPromiseRef.current;
    const promise = (async () => {
      const session = await invoke<ChatSession>("session_create", {
        kind: "chat",
        title: "架构对话",
        category: ARCH_DESIGN_CATEGORY,
      });
      updatePrefs({ sessionId: session.id });
      return session.id;
    })();
    sessionPromiseRef.current = promise;
    try {
      return await promise;
    } finally {
      if (sessionPromiseRef.current === promise) sessionPromiseRef.current = null;
    }
  }, [updatePrefs]);

  // ── 感知收集：全画布截图（视觉通道；空画布/失败时跳过）──
  const collectScreenshotSegment = useCallback(
    async (
      canvasApi: ExcalidrawImperativeAPI,
      targetSessionId: string,
    ): Promise<AnyContentSegment | null> => {
      const elements = canvasApi.getSceneElements();
      if (elements.length === 0) return null;
      try {
        const blob = await exportToBlob(canvasExportOptions([...elements], canvasApi.getFiles()));
        const imageDataBase64 = await blobToBase64(blob);
        const saved = await invoke<{ imageId: string; mimeType: string }>("save_chat_image", {
          workspaceId: targetSessionId,
          imageDataBase64,
          mimeType: "image/jpeg",
        });
        return {
          id: crypto.randomUUID(),
          type: "image",
          imageId: saved.imageId,
          mimeType: saved.mimeType,
          source: "user_paste",
        };
      } catch (error) {
        console.error("架构画布截图失败，已跳过视觉感知:", error);
        return null;
      }
    },
    [],
  );

  // ── 串行执行队列（镜像 useDispatcherActions.enqueueDispatcherRun）──
  const runQueuesRef = useRef<Map<string, Promise<void>>>(new Map());
  const enqueueArchRun = useCallback(
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
            // 发送即占位（与主聊天同款）：run 入口准备期间气泡立即可见。
            assistantPlaceholder: "正在发送…",
          }));

          const deps: DispatcherEventChannelDeps = {
            targetSessionId,
            runId,
            updateLiveSessionState,
            // 面板不展示用量明细，留空操作即可。
            refreshSessionTokenUsage: async () => {},
          };
          const onEvent = createDispatcherEventChannel(deps);

          try {
            await runner(onEvent);
          } finally {
            if (getDispatcherActiveRunId(targetSessionId) === runId) {
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
    [updateLiveSessionState],
  );

  // ── 发送：[截图段?] + [用户文本] + [快照文本段] ──
  const send = useCallback(
    async (rawText: string): Promise<boolean> => {
      const text = rawText.trim();
      if (!text) return false;
      setSendError(null);
      try {
        const targetSessionId = await ensureSession();
        // 乐观注入紧随会话就绪：截图采集（画布渲染 + 落盘）与 run 入口准备
        // 耗时期间，用户文本即时上屏。附件段不进乐观消息——权威 userMessage
        // 事件到达时整体替换，图片随权威消息出现即可。
        notifyDispatcherMessages(targetSessionId, [
          buildOptimisticUserMessage(targetSessionId, text, []),
        ]);
        const currentPrefs = prefsRef.current;
        const canvasApi = getCanvasApiRef.current();

        const segments: AnyContentSegment[] = [];
        if (currentPrefs.attachScreenshot && canvasApi) {
          const screenshot = await collectScreenshotSegment(canvasApi, targetSessionId);
          if (screenshot) segments.push(screenshot);
        }
        segments.push({ id: crypto.randomUUID(), type: "text", text });
        if (currentPrefs.attachSnapshot && canvasApi) {
          const snapshot = collectCanvasSnapshot(canvasApi);
          if (snapshot) {
            segments.push({ id: crypto.randomUUID(), type: "text", text: snapshot });
          }
        }

        const segmentsJson = JSON.stringify(segments);
        const modelLibraryId = currentPrefs.modelLibraryId ?? undefined;
        await enqueueArchRun(targetSessionId, async (onEvent) => {
          await invoke<DispatcherAgentTurn>("dispatcher_send_architecture_agent_message", {
            workspaceId: targetSessionId,
            segmentsJson,
            modelLibraryId,
            onEvent,
          });
        });
        return true;
      } catch (error) {
        console.error("架构助手消息发送失败:", error);
        const message = `架构助手执行失败：${toErrorMessage(error)}`;
        // ensureSession 失败时还没有 sessionId，live state 无处挂载——
        // 面板级 sendError 兜底，保证失败可见（调用方据此恢复输入）。
        setSendError(message);
        const targetSessionId = prefsRef.current.sessionId;
        if (targetSessionId) {
          updateLiveSessionState(targetSessionId, (state) => ({
            ...state,
            hasPendingRun: false,
            isLoading: false,
            runError: state.runError ?? message,
          }));
          // 命令 reject 且无 failed 事件时兜底清槽 + 对账（清掉乐观 pending）。
          clearDispatcherActiveRunId(targetSessionId);
          reconcileSessionMessages(targetSessionId);
        }
        return false;
      }
    },
    [collectScreenshotSegment, enqueueArchRun, ensureSession, updateLiveSessionState],
  );

  const stop = useCallback(async () => {
    const targetSessionId = prefsRef.current.sessionId;
    if (!targetSessionId) return;
    try {
      await invoke<void>("dispatcher_stop_run", { workspaceId: targetSessionId });
    } catch (error) {
      console.error("停止架构助手运行失败:", error);
    }
  }, []);

  const newConversation = useCallback(() => {
    const previousId = prefsRef.current.sessionId;
    if (previousId && getDispatcherSessionRunning(previousId)) {
      // 运行中的旧会话后端会拒绝删除（fail-closed）；若仍发起删除，
      // fire-and-forget 的失败会留下不可见、不可管理的孤儿会话——前置拦截。
      setSendError("当前对话仍在生成中，请先停止后再开新对话。");
      return;
    }
    // 先同步切换到空会话（立即可发起新对话），旧会话在后台清理。
    updatePrefs({ sessionId: null });
    if (!previousId) return;
    // 旧架构会话被排除出主列表/搜索且无管理入口：换新时必须级联删除
    //（session_delete 事务内清 DB 并回收 chat-images 文件），
    // 否则每次「新对话」都遗留不可见、不可删的孤儿会话与截图文件。
    void invoke<void>("session_delete", { sessionId: previousId }).catch((error) => {
      console.error("清理旧架构会话失败:", error);
    });
  }, [updatePrefs]);

  return {
    sessionId,
    messages,
    isRunning: isSessionRunning,
    sendError,
    prefs,
    updatePrefs,
    send,
    stop,
    newConversation,
  };
}

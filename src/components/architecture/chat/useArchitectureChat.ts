/**
 * 架构助手聊天面板的数据钩子：会话懒创建、消息历史、事件流、双重感知
 * （截图 + 结构化快照）发送、串行执行队列与停止。
 *
 * 后端入口：`dispatcher_send_architecture_agent_message`（按面板选择的视觉
 * 模型库条目构建 Agent）；执行往返由 `useArchRunListener` 在画布侧承接。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke, type Channel } from "@tauri-apps/api/core";
import type { Editor } from "tldraw";
import type {
  AnyContentSegment,
  ChatSession,
  DispatcherAgentEvent,
  DispatcherAgentTurn,
  DispatcherMessage,
} from "../../../types";
import { ARCH_DESIGN_CATEGORY } from "../../../types/architecture";
import {
  createIdleLiveSessionState,
  getDispatcherActiveRunId,
  getDispatcherLiveSessionState,
  getOrCreateDispatcherLiveSessionState,
  nextDispatcherActiveRunId,
  notifyDispatcherLiveSessionSubscribers,
  setDispatcherLiveSessionState,
  type DispatcherLiveSessionState,
} from "../../dispatcherSessionStore";
import type { LiveSessionUpdater } from "../../dispatcher-chat/useLiveSessionState";
import {
  createDispatcherEventChannel,
  type DispatcherEventChannelDeps,
} from "../../dispatcher-chat/event-channel";
import { toErrorMessage, createEmptyUsageStats } from "../../dispatcher-chat/dispatcherChatUtils";
import { useChatMessages } from "../../chat-page-v2/useChatMessages";
import { useLiveSessionStateReadonly } from "../../dispatcher-chat/useLiveSessionState";
import { collectCanvasSnapshot } from "../canvas-snapshot";
import { blobToBase64 } from "../program/arch-executor";
import {
  loadArchChatPrefs,
  saveArchChatPrefs,
  type ArchitectureChatPrefs,
} from "./architecture-chat-prefs";

export interface UseArchitectureChatOptions {
  getEditor: () => Editor | null;
}

export interface UseArchitectureChatResult {
  sessionId: string | null;
  messages: DispatcherMessage[];
  liveState: DispatcherLiveSessionState | null;
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

/** 全画布截图最长边上限（与执行器区域截图一致）。 */
const SCREENSHOT_MAX_DIM = 1600;

export function useArchitectureChat({
  getEditor,
}: UseArchitectureChatOptions): UseArchitectureChatResult {
  const [prefs, setPrefs] = useState<ArchitectureChatPrefs>(loadArchChatPrefs);
  const [sendError, setSendError] = useState<string | null>(null);
  const prefsRef = useRef(prefs);
  prefsRef.current = prefs;
  const getEditorRef = useRef(getEditor);
  getEditorRef.current = getEditor;

  const sessionId = prefs.sessionId;
  const noopResetEditing = useCallback(() => {}, []);
  const { messages } = useChatMessages(sessionId, noopResetEditing);
  const liveState = useLiveSessionStateReadonly(sessionId);

  // ── live state 更新器：直连单例 store + rAF 批量通知（防流式渲染风暴）──
  const pendingNotifyRef = useRef<{ raf: number | null; sessions: Set<string> }>({
    raf: null,
    sessions: new Set(),
  });
  useEffect(() => {
    const pending = pendingNotifyRef.current;
    return () => {
      if (pending.raf !== null) {
        cancelAnimationFrame(pending.raf);
        pending.raf = null;
      }
      pending.sessions.clear();
    };
  }, []);

  const updateLiveSessionState: LiveSessionUpdater = useCallback((targetSessionId, updater) => {
    const next = updater(getOrCreateDispatcherLiveSessionState(targetSessionId));
    setDispatcherLiveSessionState(targetSessionId, next);
    const pending = pendingNotifyRef.current;
    if (pending.sessions.has(targetSessionId)) return;
    pending.sessions.add(targetSessionId);
    if (pending.raf !== null) return;
    pending.raf = requestAnimationFrame(() => {
      for (const sid of pending.sessions) {
        const state = getDispatcherLiveSessionState(sid);
        if (state) notifyDispatcherLiveSessionSubscribers(sid, state);
      }
      pending.sessions.clear();
      pending.raf = null;
    });
  }, []);

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
      const session = await invoke<ChatSession>("chat_create_session", {
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
    async (editor: Editor, targetSessionId: string): Promise<AnyContentSegment | null> => {
      if (editor.getCurrentPageShapes().length === 0) return null;
      try {
        let minX = Infinity;
        let minY = Infinity;
        let maxX = -Infinity;
        let maxY = -Infinity;
        for (const shape of editor.getCurrentPageShapes()) {
          const bounds = editor.getShapePageBounds(shape);
          if (!bounds) continue;
          minX = Math.min(minX, bounds.minX);
          minY = Math.min(minY, bounds.minY);
          maxX = Math.max(maxX, bounds.maxX);
          maxY = Math.max(maxY, bounds.maxY);
        }
        const maxDim = Math.max(maxX - minX, maxY - minY);
        const scale = maxDim > 0 ? Math.min(1, SCREENSHOT_MAX_DIM / maxDim) : 1;
        const { blob } = await editor.toImage([], {
          format: "jpeg",
          quality: 0.8,
          background: true,
          pixelRatio: 1,
          scale,
        });
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
          const now = Date.now();
          updateLiveSessionState(targetSessionId, () => ({
            ...createIdleLiveSessionState(),
            hasPendingRun: true,
            isLoading: true,
            activeUsageStats: createEmptyUsageStats(),
            activeUsageStatsReceivedAt: now,
            usageClockNow: now,
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
        const currentPrefs = prefsRef.current;
        const editor = getEditorRef.current();

        const segments: AnyContentSegment[] = [];
        if (currentPrefs.attachScreenshot && editor) {
          const screenshot = await collectScreenshotSegment(editor, targetSessionId);
          if (screenshot) segments.push(screenshot);
        }
        segments.push({ id: crypto.randomUUID(), type: "text", text });
        if (currentPrefs.attachSnapshot && editor) {
          const snapshot = collectCanvasSnapshot(editor);
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
            runError: message,
          }));
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
    // 先同步切换到空会话（立即可发起新对话），旧会话在后台清理。
    updatePrefs({ sessionId: null });
    if (!previousId) return;
    // 旧架构会话被排除出主列表/搜索且无管理入口：换新时必须级联删除
    //（chat_delete_session 事务内清 DB 并回收 chat-images 文件），
    // 否则每次「新对话」都遗留不可见、不可删的孤儿会话与截图文件。
    void invoke<void>("chat_delete_session", { sessionId: previousId }).catch((error) => {
      console.error("清理旧架构会话失败:", error);
    });
  }, [updatePrefs]);

  return {
    sessionId,
    messages,
    liveState,
    isRunning: Boolean(liveState && (liveState.hasPendingRun || liveState.isLoading)),
    sendError,
    prefs,
    updatePrefs,
    send,
    stop,
    newConversation,
  };
}

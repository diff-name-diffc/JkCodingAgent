import { useState, useRef, useEffect, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import type { BrowserStatus } from "../../types";
import { updateLiveBrowserToolActivity } from "./live-tool-activity";
import {
  getDispatcherLiveSessionState,
  getDispatcherSessionRunning,
  getOrCreateDispatcherLiveSessionState,
  setDispatcherLiveSessionState,
  notifyDispatcherLiveSessionSubscribers,
  subscribeDispatcherLiveSession,
  subscribeDispatcherSessionRunning,
  type DispatcherLiveSessionState,
} from "../dispatcherSessionStore";

export type LiveSessionUpdater = (
  targetSessionId: string,
  updater: (state: DispatcherLiveSessionState) => DispatcherLiveSessionState,
) => void;

/**
 * Read-only bridge: subscribes to the dispatcherSessionStore singleton and
 * exposes the live state for a session id (or null when no session/state).
 * Used by the message list (streaming bubble + error block); the chat shell
 * and page wrapper deliberately do NOT subscribe — during streaming this
 * hook fires on every rAF batch, so each extra subscriber re-renders its
 * whole subtree per frame.
 *
 * 会话切换的同步性：快照绑定来源会话 id，sessionId 变化的首帧即同步读
 * store（React「渲染期调整 state」模式）——若等 paint 后的 effect 回填，
 * 从流式中的会话切走时新会话会闪现旧会话的流式状态一帧。
 */
export function useLiveSessionStateReadonly(
  sessionId: string | null,
): DispatcherLiveSessionState | null {
  const [snapshot, setSnapshot] = useState<{
    id: string | null;
    state: DispatcherLiveSessionState | null;
  }>(() => ({
    id: sessionId,
    state: sessionId ? (getDispatcherLiveSessionState(sessionId) ?? null) : null,
  }));

  if (snapshot.id !== sessionId) {
    setSnapshot({
      id: sessionId,
      state: sessionId ? (getDispatcherLiveSessionState(sessionId) ?? null) : null,
    });
  }

  useEffect(() => {
    if (!sessionId) return;
    ensureBrowserStatusBridge();
    const unsubscribe = subscribeDispatcherLiveSession(sessionId, (next) => {
      setSnapshot({ id: sessionId, state: next });
    });
    // 订阅后对齐一次，兜住渲染期读取与订阅注册之间漏掉的更新。
    setSnapshot({
      id: sessionId,
      state: getDispatcherLiveSessionState(sessionId) ?? null,
    });
    return unsubscribe;
  }, [sessionId]);

  return snapshot.state;
}

/**
 * Run-boundary boolean subscription: fires only when a run starts/ends for
 * the session, never per streaming frame. Lets page-level consumers (composer
 * send/stop mode, header loading dot) stay inert during token streaming.
 *
 * 同样用「渲染期调整」保证切换后首帧即新会话的运行态，避免从运行中的
 * 会话切走时新会话闪现一帧「停止」模式的 composer。
 */
export function useDispatcherSessionRunning(sessionId: string | null): boolean {
  const [snapshot, setSnapshot] = useState<{ id: string | null; running: boolean }>(() => ({
    id: sessionId,
    running: sessionId ? getDispatcherSessionRunning(sessionId) : false,
  }));

  if (snapshot.id !== sessionId) {
    setSnapshot({
      id: sessionId,
      running: sessionId ? getDispatcherSessionRunning(sessionId) : false,
    });
  }

  useEffect(() => {
    if (!sessionId) return;
    const unsubscribe = subscribeDispatcherSessionRunning(sessionId, (running) => {
      setSnapshot({ id: sessionId, running });
    });
    // 订阅后对齐一次，兜住渲染期读取与订阅注册之间漏掉的边界翻转。
    const current = getDispatcherSessionRunning(sessionId);
    setSnapshot((previous) =>
      previous.id === sessionId && previous.running === current
        ? previous
        : { id: sessionId, running: current },
    );
    return unsubscribe;
  }, [sessionId]);

  return snapshot.running;
}

/**
 * Store writer with rAF-batched subscriber notifications: streaming deltas
 * (~50 events/s) coalesce into one notify per frame, preventing render storms.
 * Pure writer — no React state is held here, so mounting this hook never
 * re-renders the host component.
 */
export function useLiveSessionUpdater(): LiveSessionUpdater {
  const pendingNotifyRaf = useRef<number | null>(null);
  const pendingNotifySessions = useRef(new Set<string>());

  // Clean up pending rAF on unmount
  useEffect(() => {
    const sessions = pendingNotifySessions.current;
    return () => {
      if (pendingNotifyRaf.current !== null) {
        cancelAnimationFrame(pendingNotifyRaf.current);
        pendingNotifyRaf.current = null;
      }
      sessions.clear();
    };
  }, []);

  return useCallback(
    (
      targetSessionId: string,
      updater: (state: DispatcherLiveSessionState) => DispatcherLiveSessionState,
    ) => {
      const next = updater(getOrCreateDispatcherLiveSessionState(targetSessionId));
      setDispatcherLiveSessionState(targetSessionId, next);

      if (!pendingNotifySessions.current.has(targetSessionId)) {
        pendingNotifySessions.current.add(targetSessionId);
        if (pendingNotifyRaf.current === null) {
          pendingNotifyRaf.current = requestAnimationFrame(() => {
            for (const sid of pendingNotifySessions.current) {
              const state = getDispatcherLiveSessionState(sid);
              if (state) {
                notifyDispatcherLiveSessionSubscribers(sid, state);
              }
            }
            pendingNotifySessions.current.clear();
            pendingNotifyRaf.current = null;
          });
        }
      }
    },
    [],
  );
}

// ─── browser-status → live tool activity bridge ──────────────────────────────
// 浏览器工具的实时状态（导航/加载/失败）从全局 Tauri 事件进入 live state。
// 桥接注册为进程级单例：任何聊天表面（主聊天/项目/架构面板）挂载只读订阅时
// 懒注册一次，与组件树解耦——此前它寄生在已删除的 useLiveSessionState
// hook 内，组件结构变化会静默丢失浏览器活动反馈。

let browserStatusBridgeStarted = false;

function ensureBrowserStatusBridge() {
  if (browserStatusBridgeStarted) return;
  browserStatusBridgeStarted = true;
  listen<BrowserStatus>("browser-status", (event) => {
    const status = event.payload;
    const targetSessionId = status.sessionId;
    const current = getOrCreateDispatcherLiveSessionState(targetSessionId);
    const liveToolCalls = updateLiveBrowserToolActivity(current.liveToolCalls, status);
    const assistantPlaceholder = liveToolCalls.some(
      (tool) => tool.status === "running" && tool.name.startsWith("browser_"),
    )
      ? status.message || "正在执行浏览器操作..."
      : current.assistantPlaceholder;
    const next = { ...current, liveToolCalls, assistantPlaceholder };
    setDispatcherLiveSessionState(targetSessionId, next);
    notifyDispatcherLiveSessionSubscribers(targetSessionId, next);
  }).catch((error) => {
    console.error("注册 browser-status 监听失败:", error);
    browserStatusBridgeStarted = false;
  });
}

import { useState, useRef, useEffect, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import type { BrowserStatus } from "../../types";
import { updateLiveBrowserToolActivity } from "../dispatcherChatView";
import {
  getDispatcherLiveSessionState,
  getOrCreateDispatcherLiveSessionState,
  setDispatcherLiveSessionState,
  notifyDispatcherLiveSessionSubscribers,
  subscribeDispatcherLiveSession,
  type DispatcherLiveSessionState,
} from "../dispatcherSessionStore";

export type LiveSessionUpdater = (
  targetSessionId: string,
  updater: (state: DispatcherLiveSessionState) => DispatcherLiveSessionState,
) => void;

export interface UseLiveSessionStateResult {
  liveState: DispatcherLiveSessionState;
  updateLiveSessionState: LiveSessionUpdater;
  applyLiveSessionState: (state: DispatcherLiveSessionState) => void;
}

export function useLiveSessionState(sessionId: string): UseLiveSessionStateResult {
  const [liveState, setLiveState] = useState<DispatcherLiveSessionState>(
    () => getOrCreateDispatcherLiveSessionState(sessionId),
  );

  const currentSessionIdRef = useRef(sessionId);
  currentSessionIdRef.current = sessionId;

  const pendingNotifyRaf = useRef<number | null>(null);
  const pendingNotifySessions = useRef(new Set<string>());

  const applyLiveSessionState = useCallback((state: DispatcherLiveSessionState) => {
    setLiveState(state);
  }, []);

  const updateLiveSessionState = useCallback(
    (
      targetSessionId: string,
      updater: (state: DispatcherLiveSessionState) => DispatcherLiveSessionState,
    ) => {
      const next = updater(getOrCreateDispatcherLiveSessionState(targetSessionId));
      setDispatcherLiveSessionState(targetSessionId, next);

      // Batch subscriber notifications via rAF to prevent render storms
      // during high-frequency streaming (~50 tokens/s).
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

      // Always update current session state immediately for responsiveness
      if (currentSessionIdRef.current === targetSessionId) {
        applyLiveSessionState(next);
      }
    },
    [applyLiveSessionState],
  );

  // Subscribe to external store changes + initialize on session change
  useEffect(() => {
    applyLiveSessionState(getOrCreateDispatcherLiveSessionState(sessionId));
    return subscribeDispatcherLiveSession(sessionId, applyLiveSessionState);
  }, [applyLiveSessionState, sessionId]);

  // Listen for browser status events
  useEffect(() => {
    const unlisten = listen<BrowserStatus>("browser-status", (event) => {
      const status = event.payload;
      const targetSessionId = status.sessionId;
      updateLiveSessionState(targetSessionId, (state) => ({
        ...state,
        assistantPlaceholder:
          state.liveToolCalls.some(
            (tool) => tool.status === "running" && tool.name.startsWith("browser_"),
          )
            ? status.message || "正在执行浏览器操作..."
            : state.assistantPlaceholder,
        liveToolCalls: updateLiveBrowserToolActivity(state.liveToolCalls, status),
      }));
    });

    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, [updateLiveSessionState]);

  // Usage clock timer — ticks every second while stats are active
  useEffect(() => {
    if (!liveState.activeUsageStats) return;
    const timer = window.setInterval(() => {
      setLiveState((prev) => ({ ...prev, usageClockNow: Date.now() }));
    }, 1000);
    return () => window.clearInterval(timer);
  }, [!!liveState.activeUsageStats]);

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

  return {
    liveState,
    updateLiveSessionState,
    applyLiveSessionState,
  };
}

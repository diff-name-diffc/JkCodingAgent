import { invoke } from "@tauri-apps/api/core";
import type { DispatcherMessage, DispatcherMessageWire } from "../types";
import type { AssistantThinkingBlock, AssistantTurnSegment } from "./dispatcher-chat/assistant-segments";
import type { ToolActivityItem } from "./dispatcher-chat/tool-activity";

export interface DispatcherLiveSessionState {
  hasPendingRun: boolean;
  isLoading: boolean;
  streamingSegments: AssistantTurnSegment[];
  liveThinking: AssistantThinkingBlock | null;
  liveToolCalls: ToolActivityItem[];
  assistantPlaceholder: string | null;
  runError: string | null;
}

export function createIdleLiveSessionState(): DispatcherLiveSessionState {
  return {
    hasPendingRun: false,
    isLoading: false,
    streamingSegments: [],
    liveThinking: null,
    liveToolCalls: [],
    assistantPlaceholder: null,
    runError: null,
  };
}

const dispatcherLiveSessionStates = new Map<string, DispatcherLiveSessionState>();
const dispatcherSessionRunningStates = new Map<string, boolean>();
const dispatcherActiveRunIds = new Map<string, number>();
const dispatcherLiveSessionSubscribers = new Map<
  string,
  Set<(state: DispatcherLiveSessionState) => void>
>();
const dispatcherRunningSubscribers = new Map<string, Set<(isRunning: boolean) => void>>();
const dispatcherMessageSubscribers = new Map<
  string,
  Set<(messages: DispatcherMessageBatch) => void>
>();

function isLiveSessionRunning(state: DispatcherLiveSessionState | undefined): boolean {
  return Boolean(state?.hasPendingRun || state?.isLoading);
}

function setDispatcherSessionRunningState(sessionId: string, running: boolean) {
  if (running) {
    dispatcherSessionRunningStates.set(sessionId, true);
  } else {
    dispatcherSessionRunningStates.delete(sessionId);
  }
}

export function getDispatcherLiveSessionState(sessionId: string) {
  return dispatcherLiveSessionStates.get(sessionId);
}

export function setDispatcherLiveSessionState(
  sessionId: string,
  state: DispatcherLiveSessionState,
) {
  dispatcherLiveSessionStates.set(sessionId, state);
  setDispatcherSessionRunningState(sessionId, isLiveSessionRunning(state));
}

export function getOrCreateDispatcherLiveSessionState(sessionId: string) {
  const existing = dispatcherLiveSessionStates.get(sessionId);
  if (existing) return existing;
  const created = createIdleLiveSessionState();
  dispatcherLiveSessionStates.set(sessionId, created);
  return created;
}

export function notifyDispatcherLiveSessionSubscribers(
  sessionId: string,
  state: DispatcherLiveSessionState,
) {
  dispatcherLiveSessionSubscribers.get(sessionId)?.forEach((subscriber) => subscriber(state));
  const running = isLiveSessionRunning(state);
  setDispatcherSessionRunningState(sessionId, running);
  dispatcherRunningSubscribers.get(sessionId)?.forEach((subscriber) => subscriber(running));
  cleanupIdleUnobservedSession(sessionId);
}

function hasSessionSubscribers(sessionId: string): boolean {
  return (
    (dispatcherLiveSessionSubscribers.get(sessionId)?.size ?? 0) > 0 ||
    (dispatcherMessageSubscribers.get(sessionId)?.size ?? 0) > 0 ||
    (dispatcherRunningSubscribers.get(sessionId)?.size ?? 0) > 0
  );
}

function cleanupIdleUnobservedSession(sessionId: string) {
  if (hasSessionSubscribers(sessionId) || getDispatcherSessionRunning(sessionId)) return;
  dispatcherLiveSessionStates.delete(sessionId);
  dispatcherActiveRunIds.delete(sessionId);
}

export function subscribeDispatcherLiveSession(
  sessionId: string,
  subscriber: (state: DispatcherLiveSessionState) => void,
) {
  const subscribers = dispatcherLiveSessionSubscribers.get(sessionId) ?? new Set();
  subscribers.add(subscriber);
  dispatcherLiveSessionSubscribers.set(sessionId, subscribers);
  return () => {
    subscribers.delete(subscriber);
    if (subscribers.size === 0) {
      dispatcherLiveSessionSubscribers.delete(sessionId);
      cleanupIdleUnobservedSession(sessionId);
    }
  };
}

/** 消息订阅的批次载荷：wire（后端事件/全量对账）或已归一化消息（乐观注入）。 */
export type DispatcherMessageBatch = Array<DispatcherMessageWire | DispatcherMessage>;

export function notifyDispatcherMessages(sessionId: string, messages: DispatcherMessageBatch) {
  if (messages.length === 0) return;
  dispatcherMessageSubscribers.get(sessionId)?.forEach((subscriber) => subscriber(messages));
}

export function subscribeDispatcherMessages(
  sessionId: string,
  subscriber: (messages: DispatcherMessageBatch) => void,
) {
  const subscribers = dispatcherMessageSubscribers.get(sessionId) ?? new Set();
  subscribers.add(subscriber);
  dispatcherMessageSubscribers.set(sessionId, subscribers);
  return () => {
    subscribers.delete(subscriber);
    if (subscribers.size === 0) {
      dispatcherMessageSubscribers.delete(sessionId);
      cleanupIdleUnobservedSession(sessionId);
    }
  };
}

/**
 * 运行收尾/会话记录变更的全量消息对账：拉全量消息经订阅通道分发（消费方
 * 以 mergeDispatcherMessages 合并）。此前 finished / failed / 发送 reject、
 * 断连释放与 dispatcher-session-updated 重载各自实现一份 fetch+merge，
 * 现统一为本函数单一管线。
 *
 * 竞态守卫：list_messages 在途期间若已开启新 run，过期全量快照不得推送
 * （merge 只增不删，可能把已删消息加回来）。
 */
export function reconcileSessionMessages(
  targetSessionId: string,
  expectedCount?: number,
): void {
  void invoke<DispatcherMessageWire[]>("dispatcher_list_messages", {
    workspaceId: targetSessionId,
  })
    .then((fresh) => {
      if (getDispatcherActiveRunId(targetSessionId) !== undefined) return;
      if (expectedCount !== undefined && fresh.length !== expectedCount) {
        console.warn(
          `Finished 对账不一致：后端 ${expectedCount} 条，拉取到 ${fresh.length} 条`,
        );
      }
      notifyDispatcherMessages(targetSessionId, fresh);
    })
    .catch((err) => console.error("运行收尾对账消息失败:", err));
}

export function cleanupDispatcherSession(sessionId: string) {
  dispatcherLiveSessionStates.delete(sessionId);
  dispatcherSessionRunningStates.delete(sessionId);
  dispatcherActiveRunIds.delete(sessionId);
  dispatcherLiveSessionSubscribers.delete(sessionId);
  dispatcherRunningSubscribers.delete(sessionId);
  dispatcherMessageSubscribers.delete(sessionId);
}

export function getDispatcherSessionRunning(sessionId: string): boolean {
  return (
    dispatcherSessionRunningStates.get(sessionId) ??
    isLiveSessionRunning(dispatcherLiveSessionStates.get(sessionId))
  );
}

export function withDispatcherSessionRunning<T extends { id: string; isRunning?: boolean }>(
  session: T,
): T {
  const isRunning = getDispatcherSessionRunning(session.id);
  return session.isRunning === isRunning ? session : { ...session, isRunning };
}

export function withDispatcherSessionsRunning<T extends { id: string; isRunning?: boolean }>(
  sessions: T[],
): T[] {
  return sessions.map(withDispatcherSessionRunning);
}

export function subscribeDispatcherSessionRunning(
  sessionId: string,
  subscriber: (isRunning: boolean) => void,
) {
  const subscribers = dispatcherRunningSubscribers.get(sessionId) ?? new Set();
  subscribers.add(subscriber);
  dispatcherRunningSubscribers.set(sessionId, subscribers);
  return () => {
    subscribers.delete(subscriber);
    if (subscribers.size === 0) {
      dispatcherRunningSubscribers.delete(sessionId);
      cleanupIdleUnobservedSession(sessionId);
    }
  };
}

export function getDispatcherActiveRunId(sessionId: string) {
  return dispatcherActiveRunIds.get(sessionId);
}

export function nextDispatcherActiveRunId(sessionId: string) {
  const runId = (dispatcherActiveRunIds.get(sessionId) ?? 0) + 1;
  dispatcherActiveRunIds.set(sessionId, runId);
  return runId;
}

export function clearDispatcherActiveRunId(sessionId: string) {
  dispatcherActiveRunIds.delete(sessionId);
}

import type {
  DispatcherMessage,
  DispatcherMessageUsageStats,
} from "../types";
import type {
  AssistantThinkingBlock,
  AssistantTurnSegment,
} from "./dispatcherChatView";
import type { ToolActivityItem } from "./dispatcher-chat/tool-activity";

export interface DispatcherLiveSessionState {
  hasPendingRun: boolean;
  isLoading: boolean;
  streamingSegments: AssistantTurnSegment[];
  liveThinking: AssistantThinkingBlock | null;
  liveToolCalls: ToolActivityItem[];
  assistantPlaceholder: string | null;
  runError: string | null;
  activeUsageStats: DispatcherMessageUsageStats | null;
  activeUsageStatsReceivedAt: number;
  usageClockNow: number;
}

export function createIdleLiveSessionState(): DispatcherLiveSessionState {
  const now = Date.now();
  return {
    hasPendingRun: false,
    isLoading: false,
    streamingSegments: [],
    liveThinking: null,
    liveToolCalls: [],
    assistantPlaceholder: null,
    runError: null,
    activeUsageStats: null,
    activeUsageStatsReceivedAt: now,
    usageClockNow: now,
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
  Set<(messages: DispatcherMessage[]) => void>
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

export function notifyDispatcherMessages(sessionId: string, messages: DispatcherMessage[]) {
  if (messages.length === 0) return;
  dispatcherMessageSubscribers.get(sessionId)?.forEach((subscriber) => subscriber(messages));
}

export function subscribeDispatcherMessages(
  sessionId: string,
  subscriber: (messages: DispatcherMessage[]) => void,
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

export function cleanupDispatcherSession(sessionId: string) {
  dispatcherLiveSessionStates.delete(sessionId);
  dispatcherSessionRunningStates.delete(sessionId);
  dispatcherActiveRunIds.delete(sessionId);
  dispatcherLiveSessionSubscribers.delete(sessionId);
  dispatcherRunningSubscribers.delete(sessionId);
  dispatcherMessageSubscribers.delete(sessionId);
}

export function gcDispatcherSessions() {
  for (const id of dispatcherLiveSessionStates.keys()) {
    cleanupIdleUnobservedSession(id);
  }
}

export function getDispatcherSessionRunning(sessionId: string): boolean {
  return dispatcherSessionRunningStates.get(sessionId) ?? isLiveSessionRunning(dispatcherLiveSessionStates.get(sessionId));
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

export function setDispatcherActiveRunId(sessionId: string, runId: number) {
  dispatcherActiveRunIds.set(sessionId, runId);
}

export function nextDispatcherActiveRunId(sessionId: string) {
  const runId = (dispatcherActiveRunIds.get(sessionId) ?? 0) + 1;
  dispatcherActiveRunIds.set(sessionId, runId);
  return runId;
}

export function clearDispatcherActiveRunId(sessionId: string) {
  dispatcherActiveRunIds.delete(sessionId);
}

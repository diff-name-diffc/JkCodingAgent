import type {
  DispatcherMessage,
  DispatcherMessageUsageStats,
} from "../types";
import type {
  AssistantThinkingBlock,
  AssistantTurnSegment,
} from "./dispatcherChatView";
import type { ToolActivityItem } from "./ToolActivityBubble";

export interface PendingDispatchApproval {
  dispatchId: string;
  agent: "claude" | "codex";
  description: string;
  taskPrompt: string;
  permissionMode: string;
}

export interface DispatcherLiveSessionState {
  hasPendingRun: boolean;
  isLoading: boolean;
  streamingSegments: AssistantTurnSegment[];
  liveThinking: AssistantThinkingBlock | null;
  liveToolCalls: ToolActivityItem[];
  assistantPlaceholder: string | null;
  runError: string | null;
  pendingDispatches: PendingDispatchApproval[];
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
    pendingDispatches: [],
    activeUsageStats: null,
    activeUsageStatsReceivedAt: now,
    usageClockNow: now,
  };
}

const dispatcherLiveSessionStates = new Map<string, DispatcherLiveSessionState>();
const dispatcherActiveRunIds = new Map<string, number>();
const dispatcherLiveSessionSubscribers = new Map<
  string,
  Set<(state: DispatcherLiveSessionState) => void>
>();
const dispatcherMessageSubscribers = new Map<
  string,
  Set<(messages: DispatcherMessage[]) => void>
>();

function isLiveSessionRunning(state: DispatcherLiveSessionState | undefined): boolean {
  return Boolean(state?.hasPendingRun || state?.isLoading);
}

export function getDispatcherLiveSessionState(sessionId: string) {
  return dispatcherLiveSessionStates.get(sessionId);
}

export function setDispatcherLiveSessionState(
  sessionId: string,
  state: DispatcherLiveSessionState,
) {
  dispatcherLiveSessionStates.set(sessionId, state);
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
      if ((dispatcherMessageSubscribers.get(sessionId)?.size ?? 0) === 0) {
        dispatcherLiveSessionStates.delete(sessionId);
        dispatcherActiveRunIds.delete(sessionId);
      }
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
      if ((dispatcherLiveSessionSubscribers.get(sessionId)?.size ?? 0) === 0) {
        dispatcherLiveSessionStates.delete(sessionId);
        dispatcherActiveRunIds.delete(sessionId);
      }
    }
  };
}

export function cleanupDispatcherSession(sessionId: string) {
  dispatcherLiveSessionStates.delete(sessionId);
  dispatcherActiveRunIds.delete(sessionId);
  dispatcherLiveSessionSubscribers.delete(sessionId);
  dispatcherMessageSubscribers.delete(sessionId);
}

export function gcDispatcherSessions() {
  for (const id of dispatcherLiveSessionStates.keys()) {
    const hasSubscribers =
      (dispatcherLiveSessionSubscribers.get(id)?.size ?? 0) > 0 ||
      (dispatcherMessageSubscribers.get(id)?.size ?? 0) > 0;
    if (!hasSubscribers) {
      dispatcherLiveSessionStates.delete(id);
      dispatcherActiveRunIds.delete(id);
    }
  }
}

export function getDispatcherSessionRunning(sessionId: string): boolean {
  return isLiveSessionRunning(dispatcherLiveSessionStates.get(sessionId));
}

export function subscribeDispatcherSessionRunning(
  sessionId: string,
  subscriber: (isRunning: boolean) => void,
) {
  return subscribeDispatcherLiveSession(sessionId, (state) => {
    subscriber(isLiveSessionRunning(state));
  });
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

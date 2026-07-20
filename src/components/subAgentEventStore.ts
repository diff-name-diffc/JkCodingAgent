import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { SubAgentEvent, SubAgentEventPayload, SubAgentUsage } from "../types";

export interface EventLine {
  id: string;
  type: string;
  text: string;
  timestamp: number;
}

export type SubAgentPhase =
  | "initializing"
  | "thinking"
  | "tool_calling"
  | "generating"
  | "completed"
  | "failed";

export interface SubAgentToolCall {
  id: string;
  toolName: string;
  arguments: Record<string, unknown>;
  resultPreview?: string;
  startedAt: number;
  finishedAt?: number;
  durationMs?: number;
  status: "running" | "completed" | "failed";
}

export interface SubAgentSession {
  agentId: string;
  name: string;
  task: string;
  responseText: string;
  progressMessages: SubAgentProgressMessage[];
  events: EventLine[];
  elapsed: number;
  status: "running" | "completed" | "failed";
  phase: SubAgentPhase;
  toolCalls: SubAgentToolCall[];
  finishedResult?: string;
  finishedError?: string;
  tokenUsage?: SubAgentUsage;
  usageReceivedAt?: number;
  iterations?: number;
}

export interface SubAgentProgressMessage {
  id: string;
  agentId: string;
  agentName: string;
  text: string;
  timestamp: number;
}

type SessionMap = Record<string, SubAgentSession>;

type Subscriber = (snapshot: Record<string, SessionMap>) => void;

const store: Record<string, SessionMap> = {};
const starts: Record<string, number> = {};
const subscribers = new Set<Subscriber>();
let listenerRegistered = false;

function keyFor(sessionId: string, toolCallId: string): string {
  return `${sessionId}:${toolCallId}`;
}

function snapshotForSession(sessionId: string): SessionMap {
  return store[sessionId] ?? {};
}

function notify(): void {
  for (const sub of subscribers) {
    sub(structuredClone(store));
  }
}

function buildEventText(eventType: string, data: SubAgentEventPayload["data"]): string {
  if (eventType === "Started") {
    return `任务：${data.task ?? ""}`;
  }
  if (eventType === "ToolStarted") {
    const args = data.arguments ?? {};
    const argsPreview = JSON.stringify(args).slice(0, 100);
    return `▶ ${data.toolName ?? ""}(${argsPreview})`;
  }
  if (eventType === "ToolFinished") {
    const preview = (data.resultPreview ?? "").slice(0, 80);
    return `◀ ${data.toolName ?? ""}: ${preview}`;
  }
  if (eventType === "Progress") {
    return `进度通知：${data.message ?? ""}`;
  }
  if (eventType === "llmDelta") {
    return `响应片段：${(data.delta ?? "").slice(0, 120)}`;
  }
  if (eventType === "Finished") {
    const totalTokens = data.tokenUsage?.totalTokens ?? 0;
    return `✅ 完成 · ${data.iterations} 轮 · ${Math.round((data.elapsedMs ?? 0) / 1000)}s · ${totalTokens} tokens`;
  }
  if (eventType === "Failed") {
    return `❌ 失败：${data.error ?? ""}`;
  }
  return "";
}

function applySubAgentEvent(payload: SubAgentEventPayload, notifyAfter = true): void {
    const { sessionId, toolCallId, event: eventType, data } = payload;
    const agentId = data.agentId ?? "unknown";
    const now = payload.timestampMs || Date.now();

    if (!store[sessionId]) {
      store[sessionId] = {};
    }
    const sessionMap = store[sessionId];
    const existing = sessionMap[toolCallId];
    const storeKey = keyFor(sessionId, toolCallId);

    let name: string;
    let task: string;
    let status: "running" | "completed" | "failed";
    let elapsed: number;
    let phase: SubAgentPhase;
    let toolCalls: SubAgentToolCall[];
    let finishedResult: string | undefined;
    let finishedError: string | undefined;
    let tokenUsage: SubAgentUsage | undefined;
    let usageReceivedAt: number | undefined;
    let iterations: number | undefined;

    if (eventType === "Started") {
      name = data.agentName ?? agentId;
      task = data.task ?? "";
      status = "running";
      elapsed = 0;
      phase = "initializing";
      toolCalls = [];
      usageReceivedAt = undefined;
      starts[storeKey] = now;
    } else if (eventType === "Finished") {
      name = existing?.name ?? agentId;
      task = existing?.task ?? "";
      status = "completed";
      elapsed = data.elapsedMs ?? 0;
      phase = "completed";
      toolCalls = existing?.toolCalls ?? [];
      finishedResult = data.result ?? existing?.finishedResult;
      tokenUsage = data.tokenUsage ?? existing?.tokenUsage;
      iterations = data.iterations ?? existing?.iterations;
    } else if (eventType === "Failed") {
      name = existing?.name ?? agentId;
      task = existing?.task ?? "";
      status = "failed";
      elapsed = now - (starts[storeKey] ?? now);
      phase = "failed";
      toolCalls = existing?.toolCalls ?? [];
      finishedError = data.error ?? existing?.finishedError;
    } else {
      name = existing?.name ?? agentId;
      task = existing?.task ?? "";
      status = existing?.status ?? "running";
      elapsed = now - (starts[storeKey] ?? now);
      toolCalls = existing?.toolCalls ? [...existing.toolCalls] : [];

      if (eventType === "ToolStarted") {
        phase = "tool_calling";
        const toolCallId = `${agentId}-${data.toolName}-${now}`;
        toolCalls.push({
          id: toolCallId,
          toolName: data.toolName ?? "",
          arguments: data.arguments ?? {},
          startedAt: now,
          status: "running",
        });
      } else if (eventType === "ToolFinished") {
        // Keep current phase, but update the last running tool call
        phase = existing?.phase ?? "tool_calling";
        const lastRunning = [...toolCalls].reverse().find(
          (tc) => tc.status === "running" && tc.toolName === (data.toolName ?? "")
        );
        if (lastRunning) {
          const idx = toolCalls.findIndex((tc) => tc.id === lastRunning.id);
          if (idx !== -1) {
            toolCalls[idx] = {
              ...toolCalls[idx],
              resultPreview: data.resultPreview,
              finishedAt: now,
              durationMs: now - toolCalls[idx].startedAt,
              status: "completed",
            };
          }
        }
      } else if (eventType === "llmDelta") {
        // If we have tool calls, we're generating the final response; otherwise thinking
        phase = toolCalls.length > 0 ? "generating" : "thinking";
      } else {
        phase = existing?.phase ?? "initializing";
      }

      finishedResult = existing?.finishedResult;
      finishedError = existing?.finishedError;
      tokenUsage = existing?.tokenUsage;
      iterations = existing?.iterations;
    }

    // UsageUpdated is a lightweight telemetry event: update token/elapsed
    // without adding an event line or changing phase.
    if (eventType === "UsageUpdated") {
      tokenUsage = data.tokenUsage ?? existing?.tokenUsage;
      elapsed = data.elapsedMs ?? elapsed;
      usageReceivedAt = now;
    }

    const text = buildEventText(eventType, data);
    const eventId = `${agentId}-${now}-${Math.random().toString(36).slice(2, 6)}`;
    const newEvent: EventLine = { id: eventId, type: eventType, text, timestamp: now };
    const events = text ? [...(existing?.events ?? []), newEvent].slice(-50) : existing?.events ?? [];
    const responseText =
      eventType === "llmDelta"
        ? `${existing?.responseText ?? ""}${data.delta ?? ""}`
        : eventType === "Started"
          ? ""
          : existing?.responseText ?? "";
    const progressMessages =
      eventType === "Progress" && data.message?.trim()
        ? [
            ...(existing?.progressMessages ?? []),
            {
              id: eventId,
              agentId,
              agentName: name,
              text: data.message.trim(),
              timestamp: now,
            },
          ].slice(-20)
        : eventType === "Started"
          ? []
          : existing?.progressMessages ?? [];

    sessionMap[toolCallId] = {
      agentId,
      name,
      task,
      responseText,
      progressMessages,
      events,
      elapsed,
      status,
      phase,
      toolCalls,
      finishedResult,
      finishedError,
      tokenUsage,
      usageReceivedAt,
      iterations,
    };
    if (notifyAfter) notify();
}

function registerGlobalListener(): void {
  if (listenerRegistered) return;
  listenerRegistered = true;

  listen<SubAgentEventPayload>("sub-agent-event", (event) => {
    applySubAgentEvent(event.payload);
  });
}

export function useSubAgentSessions(sessionId: string): SessionMap {
  const [snapshot, setSnapshot] = useState<SessionMap>(() => snapshotForSession(sessionId));

  useEffect(() => {
    registerGlobalListener();
    setSnapshot(snapshotForSession(sessionId));

    const subscriber: Subscriber = (fullStore) => {
      setSnapshot(fullStore[sessionId] ?? {});
    };
    subscribers.add(subscriber);
    return () => {
      subscribers.delete(subscriber);
    };
  }, [sessionId]);

  return snapshot;
}

export function getSubAgentSession(
  sessionId: string,
  toolCallId: string,
): SubAgentSession | null {
  return store[sessionId]?.[toolCallId] ?? null;
}

export function hydrateSubAgentTrace(
  sessionId: string,
  toolCallId: string,
  events: SubAgentEvent[],
): SubAgentSession | null {
  if (!store[sessionId]) store[sessionId] = {};
  delete store[sessionId][toolCallId];
  delete starts[keyFor(sessionId, toolCallId)];
  for (const event of events) {
    applySubAgentEvent({ sessionId, toolCallId, timestampMs: Date.now(), ...event }, false);
  }
  notify();
  return getSubAgentSession(sessionId, toolCallId);
}

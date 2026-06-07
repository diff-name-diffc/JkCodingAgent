import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { SubAgentEventPayload } from "../types";

export interface EventLine {
  id: string;
  type: string;
  text: string;
  timestamp: number;
}

export interface SubAgentSession {
  agentId: string;
  name: string;
  task: string;
  events: EventLine[];
  elapsed: number;
  status: "running" | "completed" | "failed";
}

type SessionMap = Record<string, SubAgentSession>;

type Subscriber = (snapshot: Record<string, SessionMap>) => void;

const store: Record<string, SessionMap> = {};
const starts: Record<string, number> = {};
const subscribers = new Set<Subscriber>();
let listenerRegistered = false;

function keyFor(sessionId: string, agentId: string): string {
  return `${sessionId}:${agentId}`;
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
  if (eventType === "LlmDelta") {
    return `💬 ${(data.delta ?? "").slice(0, 120)}`;
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

function registerGlobalListener(): void {
  if (listenerRegistered) return;
  listenerRegistered = true;

  listen<SubAgentEventPayload>("sub-agent-event", (event) => {
    const payload = event.payload;
    const { sessionId, event: eventType, data } = payload;
    const agentId = data.agentId ?? "unknown";
    const now = Date.now();

    if (!store[sessionId]) {
      store[sessionId] = {};
    }
    const sessionMap = store[sessionId];
    const existing = sessionMap[agentId];
    const storeKey = keyFor(sessionId, agentId);

    let name: string;
    let task: string;
    let status: "running" | "completed" | "failed";
    let elapsed: number;

    if (eventType === "Started") {
      name = data.agentName ?? agentId;
      task = data.task ?? "";
      status = "running";
      elapsed = 0;
      starts[storeKey] = now;
    } else if (eventType === "Finished") {
      name = existing?.name ?? agentId;
      task = existing?.task ?? "";
      status = "completed";
      elapsed = data.elapsedMs ?? 0;
    } else if (eventType === "Failed") {
      name = existing?.name ?? agentId;
      task = existing?.task ?? "";
      status = "failed";
      elapsed = now - (starts[storeKey] ?? now);
    } else {
      name = existing?.name ?? agentId;
      task = existing?.task ?? "";
      status = existing?.status ?? "running";
      elapsed = now - (starts[storeKey] ?? now);
    }

    const text = buildEventText(eventType, data);
    const eventId = `${agentId}-${now}-${Math.random().toString(36).slice(2, 6)}`;
    const newEvent: EventLine = { id: eventId, type: eventType, text, timestamp: now };
    const events = [...(existing?.events ?? []), newEvent].slice(-50);

    sessionMap[agentId] = { agentId, name, task, events, elapsed, status };
    notify();
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

export function extractAgentIdsFromToolInput(input: string | undefined): string | null {
  if (!input) return null;
  try {
    const parsed = JSON.parse(input);
    const id = parsed?.agent_id;
    return typeof id === "string" ? id : null;
  } catch {
    return null;
  }
}

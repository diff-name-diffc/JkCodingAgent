import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  GraphNodeRunRecord,
  GraphPlanRecord,
  GraphPlanUpdatedPayload,
  GraphRunEventPayload,
} from "../../types";

/**
 * 图编排实时 store（模块级单例）。
 *
 * 模式照搬 subAgentEventStore.ts：Tauri listen 惰性注册一次、事件折叠成
 * 内存快照、订阅广播。两点差异：
 *  - 高频 nodeOutputDelta 走 ~100ms 节流通知（CLI 节点行流式输出）；
 *  - 不做 structuredClone——快照原地更新，订阅者凭单调 version 重渲染，
 *    避免 MB 级输出缓冲被反复克隆。
 */

export interface GraphPlanSnapshot {
  plan: GraphPlanRecord | null;
  /** claude/codex 节点实时输出缓冲（nodeOutputDelta 累积，节点终结后清除）。 */
  liveOutputs: Record<string, string>;
  /** 最近一次图运行事件（面板状态提示用）。 */
  lastEvent: GraphRunEventPayload | null;
}

type Subscriber = (version: number) => void;

const EMPTY_SNAPSHOT: GraphPlanSnapshot = {
  plan: null,
  liveOutputs: {},
  lastEvent: null,
};

const snapshots = new Map<string, GraphPlanSnapshot>();
const subscribers = new Set<Subscriber>();
let listenerRegistered = false;
let version = 0;
let notifyTimer: number | null = null;

function ensureSnapshot(planId: string): GraphPlanSnapshot {
  let snapshot = snapshots.get(planId);
  if (!snapshot) {
    snapshot = { plan: null, liveOutputs: {}, lastEvent: null };
    snapshots.set(planId, snapshot);
  }
  return snapshot;
}

function notify(): void {
  version += 1;
  for (const subscriber of subscribers) {
    subscriber(version);
  }
}

/** 高频事件（nodeOutputDelta）的节流通知：100ms 内合并为一次。 */
function notifyThrottled(): void {
  if (notifyTimer !== null) return;
  notifyTimer = window.setTimeout(() => {
    notifyTimer = null;
    notify();
  }, 100);
}

function patchPlan(
  snapshot: GraphPlanSnapshot,
  patch: Partial<GraphPlanRecord>,
): void {
  if (!snapshot.plan) return;
  snapshot.plan = { ...snapshot.plan, ...patch };
}

function upsertNodeRun(
  snapshot: GraphPlanSnapshot,
  nodeId: string,
  patch: Partial<GraphNodeRunRecord> & Pick<GraphNodeRunRecord, "status">,
): void {
  const plan = snapshot.plan;
  if (!plan) return;
  const index = plan.nodeRuns.findIndex((run) => run.nodeId === nodeId);
  if (index === -1) return;
  const nextRuns = [...plan.nodeRuns];
  nextRuns[index] = { ...nextRuns[index], ...patch };
  snapshot.plan = { ...plan, nodeRuns: nextRuns };
}

function applyGraphRunEvent(payload: GraphRunEventPayload): void {
  const snapshot = ensureSnapshot(payload.planId);
  snapshot.lastEvent = payload;
  const now = payload.timestampMs || Date.now();

  switch (payload.event) {
    case "runStarted":
      patchPlan(snapshot, { status: "running" });
      notify();
      break;
    case "nodeStarted":
      upsertNodeRun(snapshot, payload.data.nodeId, {
        status: "running",
        agentKind: payload.data.agentKind,
        agentId: payload.data.agentId,
        inputText: payload.data.input,
        outputText: "",
        errorText: null,
        startedAt: now,
        finishedAt: null,
        durationMs: null,
      });
      notify();
      break;
    case "nodeOutputDelta":
      snapshot.liveOutputs[payload.data.nodeId] =
        (snapshot.liveOutputs[payload.data.nodeId] ?? "") + payload.data.delta;
      notifyThrottled();
      break;
    case "nodeFinished":
      upsertNodeRun(snapshot, payload.data.nodeId, {
        status: "succeeded",
        outputText: payload.data.output,
        affectedFiles: payload.data.affectedFiles,
        finishedAt: now,
        durationMs: payload.data.durationMs,
      });
      delete snapshot.liveOutputs[payload.data.nodeId];
      notify();
      break;
    case "nodeFailed":
      upsertNodeRun(snapshot, payload.data.nodeId, {
        status: "failed",
        errorText: payload.data.error,
        affectedFiles: payload.data.affectedFiles,
        finishedAt: now,
        durationMs: payload.data.durationMs,
      });
      delete snapshot.liveOutputs[payload.data.nodeId];
      notify();
      break;
    case "nodeSkipped":
      upsertNodeRun(snapshot, payload.data.nodeId, {
        status: "skipped",
        finishedAt: now,
      });
      notify();
      break;
    case "stateUpdated":
      patchPlan(snapshot, { stateJson: JSON.stringify(payload.data.state ?? {}) });
      notify();
      break;
    case "runFinished":
      patchPlan(snapshot, {
        status: payload.data.failedNodes.length > 0 ? "failed" : "completed",
        stateJson: JSON.stringify(payload.data.state ?? {}),
      });
      // 终态回源：拿到权威 node runs（含持久化的 outputText/durationMs）。
      void hydrateGraphPlan(payload.planId);
      notify();
      break;
    case "runFailed":
      patchPlan(snapshot, { status: "failed" });
      void hydrateGraphPlan(payload.planId);
      notify();
      break;
    case "runCancelled":
      patchPlan(snapshot, { status: "cancelled" });
      void hydrateGraphPlan(payload.planId);
      notify();
      break;
  }
}

/** 从后端拉取权威计划记录（含 node runs + state）覆盖内存快照。 */
export async function hydrateGraphPlan(planId: string): Promise<GraphPlanRecord | null> {
  const snapshot = ensureSnapshot(planId);
  try {
    const plan = await invoke<GraphPlanRecord>("graph_plan_get", { planId });
    snapshot.plan = plan;
    // 权威记录里已终结的节点，清掉对应实时输出缓冲。
    for (const run of plan.nodeRuns) {
      if (run.status !== "running" && run.status !== "pending") {
        delete snapshot.liveOutputs[run.nodeId];
      }
    }
    notify();
    return plan;
  } catch (err) {
    console.error("加载图计划失败:", err);
    return null;
  }
}

function registerGlobalListener(): void {
  if (listenerRegistered) return;
  listenerRegistered = true;

  listen<GraphPlanUpdatedPayload>("graph-plan-updated", (event) => {
    void hydrateGraphPlan(event.payload.planId);
  });
  listen<GraphRunEventPayload>("graph-run-event", (event) => {
    applyGraphRunEvent(event.payload);
  });
}

export function getGraphPlanSnapshot(planId: string): GraphPlanSnapshot {
  return snapshots.get(planId) ?? EMPTY_SNAPSHOT;
}

/**
 * 订阅某个计划的实时快照。planId 为 null 时返回空快照。
 * 首次订阅时自动 hydrate（graph_plan_get）做历史回放。
 */
export function useGraphPlan(planId: string | null): GraphPlanSnapshot {
  const [, setVersionSeen] = useState(0);

  useEffect(() => {
    registerGlobalListener();
    const subscriber: Subscriber = (next) => setVersionSeen(next);
    subscribers.add(subscriber);
    return () => {
      subscribers.delete(subscriber);
    };
  }, []);

  useEffect(() => {
    if (!planId) return;
    registerGlobalListener();
    if (!snapshots.get(planId)?.plan) {
      void hydrateGraphPlan(planId);
    }
  }, [planId]);

  return planId ? getGraphPlanSnapshot(planId) : EMPTY_SNAPSHOT;
}

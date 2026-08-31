import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AgentActivity,
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
 *  - 高频 nodeOutputDelta 走 ~100ms 节流通知（PI SDK 文本流）；
 *  - 不做 structuredClone——快照原地更新，订阅者凭单调 version 重渲染，
 *    避免 MB 级输出缓冲被反复克隆。
 */

export interface GraphPlanSnapshot {
  plan: GraphPlanRecord | null;
  /** PI Agent 文本 delta 的实时缓冲。 */
  liveOutputs: Record<string, string>;
  liveActivities: Record<string, AgentActivity[]>;
  /** 最近一次图运行事件（面板状态提示用）。 */
  lastEvent: GraphRunEventPayload | null;
  /** 高危写检查点暂停标记（plan 仍是 running，但等待用户恢复）。 */
  paused: boolean;
  pausedNodeId: string | null;
}

type Subscriber = (version: number) => void;

const EMPTY_SNAPSHOT: GraphPlanSnapshot = {
  plan: null,
  liveOutputs: {},
  liveActivities: {},
  lastEvent: null,
  paused: false,
  pausedNodeId: null,
};

const snapshots = new Map<string, GraphPlanSnapshot>();
/** planId → workspaceId（sessionId）映射，供按会话清理快照。 */
const planWorkspaces = new Map<string, string>();
const subscribers = new Set<Subscriber>();
let listenerRegistered = false;
let version = 0;
let notifyTimer: number | null = null;

/** 内存上限：最多保留最近 N 个计划的快照，超出时淘汰最旧计划。 */
const MAX_TRACKED_PLANS = 50;

/**
 * 已清理会话的墓碑：删除会话后，Tauri IPC 队列里可能仍积压着该会话的
 * 图事件（高频 nodeOutputDelta / graph-plan-updated），晚到的事件会经
 * ensureSnapshot 重建快照，让清理失效。记录近期被清理的会话 id，
 * 晚到事件直接丢弃。集合封顶，先进先出。
 */
const cleanedSessionIds: string[] = [];
const MAX_CLEANED_SESSIONS = 128;

function rememberCleanedSession(sessionId: string): void {
  if (cleanedSessionIds.includes(sessionId)) return;
  cleanedSessionIds.push(sessionId);
  if (cleanedSessionIds.length > MAX_CLEANED_SESSIONS) cleanedSessionIds.shift();
}

function isCleanedSession(sessionId: string): boolean {
  return cleanedSessionIds.includes(sessionId);
}

/** 串行执行异步任务；单次失败不会污染后续队列。 */
export function createSerialTaskQueue() {
  let tail: Promise<unknown> = Promise.resolve();
  return function enqueue<T>(task: () => Promise<T>): Promise<T> {
    const result = tail.then(task);
    tail = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  };
}

/**
 * 淘汰目标按插入序优先取已终结的计划：running / paused 的计划仍在接收事件
 * 或正被查看，淘汰它们会丢失已累积的流式输出且视图不会自动回源。
 */
function evictOldestPlan(currentPlanId: string): void {
  let target: string | undefined;
  for (const [key, snapshot] of snapshots) {
    if (key === currentPlanId) continue;
    if (!snapshot.paused && snapshot.plan?.status !== "running") {
      target = key;
      break;
    }
  }
  target ??= [...snapshots.keys()].find((key) => key !== currentPlanId);
  if (target !== undefined) {
    snapshots.delete(target);
    planWorkspaces.delete(target);
  }
}

function ensureSnapshot(planId: string): GraphPlanSnapshot {
  let snapshot = snapshots.get(planId);
  if (!snapshot) {
    snapshot = { plan: null, liveOutputs: {}, liveActivities: {}, lastEvent: null, paused: false, pausedNodeId: null };
    snapshots.set(planId, snapshot);
    if (snapshots.size > MAX_TRACKED_PLANS) {
      evictOldestPlan(planId);
    }
  }
  return snapshot;
}

/**
 * 删除会话（或会话所属项目被删除）时调用，清理该会话全部图计划的内存快照。
 */
export function cleanupGraphPlansForSession(sessionId: string): void {
  rememberCleanedSession(sessionId);
  let removed = false;
  for (const [planId, snapshot] of snapshots) {
    if (planWorkspaces.get(planId) === sessionId || snapshot.plan?.workspaceId === sessionId) {
      snapshots.delete(planId);
      planWorkspaces.delete(planId);
      removed = true;
    }
  }
  if (removed) notify();
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

interface GraphReduceEffect {
  notification: "immediate" | "throttled";
  hydrate: boolean;
}

/** 纯事件 reducer；网络回源与通知调度由外层 store 处理。 */
export function reduceGraphRunEvent(
  snapshot: GraphPlanSnapshot,
  payload: GraphRunEventPayload,
): GraphReduceEffect {
  snapshot.lastEvent = payload;
  const now = payload.timestampMs || Date.now();

  switch (payload.event) {
    case "runStarted":
      patchPlan(snapshot, { status: "running" });
      snapshot.liveOutputs = {};
      snapshot.liveActivities = {};
      snapshot.paused = false;
      snapshot.pausedNodeId = null;
      return { notification: "immediate", hydrate: true };
    case "nodeStarted":
      upsertNodeRun(snapshot, payload.data.nodeId, {
        status: "running",
        runId: payload.runId,
        modelRef: payload.data.modelRef,
        modelLabel: payload.data.modelLabel,
        phase: "starting",
        inputText: payload.data.input,
        outputText: "",
        errorText: null,
        startedAt: now,
        finishedAt: null,
        durationMs: null,
      });
      return { notification: "immediate", hydrate: false };
    case "nodePhaseChanged":
      upsertNodeRun(snapshot, payload.data.nodeId, {
        status: "running",
        phase: payload.data.phase,
      });
      return { notification: "throttled", hydrate: false };
    case "nodeOutputDelta":
      snapshot.liveOutputs[payload.data.nodeId] =
        (snapshot.liveOutputs[payload.data.nodeId] ?? "") + payload.data.delta;
      return { notification: "throttled", hydrate: false };
    case "nodeActivity": {
      const activities = snapshot.liveActivities[payload.data.nodeId] ?? [];
      const index = activities.findIndex((item) => item.id === payload.data.activity.id);
      if (index < 0 && payload.data.activity.kind === "tool_call" && payload.data.activity.status === "started") {
        const current = snapshot.plan?.nodeRuns.find((run) => run.nodeId === payload.data.nodeId);
        upsertNodeRun(snapshot, payload.data.nodeId, {
          status: "running",
          toolCallCount: (current?.toolCallCount ?? 0) + 1,
        });
      }
      snapshot.liveActivities[payload.data.nodeId] = index < 0
        ? [...activities, payload.data.activity]
        : activities.map((item, itemIndex) => itemIndex === index ? payload.data.activity : item);
      return { notification: "throttled", hydrate: false };
    }
    case "nodeFinished":
      upsertNodeRun(snapshot, payload.data.nodeId, {
        status: "succeeded",
        phase: "finalizing",
        outputText: payload.data.output,
        affectedFiles: payload.data.affectedFiles,
        finishedAt: now,
        durationMs: payload.data.durationMs,
      });
      delete snapshot.liveOutputs[payload.data.nodeId];
      return { notification: "immediate", hydrate: false };
    case "nodeFailed":
      upsertNodeRun(snapshot, payload.data.nodeId, {
        status: "failed",
        phase: "finalizing",
        errorText: payload.data.error,
        affectedFiles: payload.data.affectedFiles,
        finishedAt: now,
        durationMs: payload.data.durationMs,
      });
      delete snapshot.liveOutputs[payload.data.nodeId];
      return { notification: "immediate", hydrate: false };
    case "nodeSkipped":
      upsertNodeRun(snapshot, payload.data.nodeId, {
        status: "skipped",
        finishedAt: now,
      });
      return { notification: "immediate", hydrate: false };
    case "nodeCancelled":
      // 与 nodeSkipped（上游失败被跳过）区分：展示层按 cancelled 状态
      // 渲染「取消」文案与统计（NODE_STATUS_META / GraphPanelHeader）。
      upsertNodeRun(snapshot, payload.data.nodeId, {
        status: "cancelled",
        finishedAt: now,
      });
      return { notification: "immediate", hydrate: false };
    case "stateUpdated":
      patchPlan(snapshot, { stateJson: JSON.stringify(payload.data.state ?? {}) });
      return { notification: "immediate", hydrate: false };
    case "runPaused":
      snapshot.paused = true;
      snapshot.pausedNodeId = payload.data.nodeId;
      return { notification: "immediate", hydrate: false };
    case "runResumed":
      snapshot.paused = false;
      snapshot.pausedNodeId = null;
      return { notification: "immediate", hydrate: false };
    case "runFinished":
      patchPlan(snapshot, {
        status: payload.data.failedNodes.length > 0 ? "failed" : "completed",
        stateJson: JSON.stringify(payload.data.state ?? {}),
      });
      snapshot.paused = false;
      snapshot.pausedNodeId = null;
      // 终态回源：拿到权威 node runs（含持久化的 outputText/durationMs）。
      return { notification: "immediate", hydrate: true };
    case "runFailed":
      patchPlan(snapshot, { status: "failed" });
      snapshot.paused = false;
      snapshot.pausedNodeId = null;
      return { notification: "immediate", hydrate: true };
    case "runCancelled":
      patchPlan(snapshot, { status: "cancelled" });
      snapshot.paused = false;
      snapshot.pausedNodeId = null;
      return { notification: "immediate", hydrate: true };
  }
}

function applyGraphRunEvent(payload: GraphRunEventPayload): void {
  // 会话已删除：丢弃积压的晚到事件，避免重建刚被清理的快照。
  if (isCleanedSession(payload.workspaceId)) return;
  planWorkspaces.set(payload.planId, payload.workspaceId);
  const effect = reduceGraphRunEvent(ensureSnapshot(payload.planId), payload);
  if (effect.hydrate) void hydrateGraphPlan(payload.planId);
  if (effect.notification === "throttled") notifyThrottled();
  else notify();
}

/** 从后端拉取权威计划记录（含 node runs + state）覆盖内存快照。 */
export async function hydrateGraphPlan(planId: string): Promise<GraphPlanRecord | null> {
  const snapshot = ensureSnapshot(planId);
  try {
    const plan = await invoke<GraphPlanRecord>("graph_plan_get", { planId });
    // 拉取期间会话被删除：不再回填，保持清理结果。
    if (isCleanedSession(plan.workspaceId)) return null;
    snapshot.plan = plan;
    planWorkspaces.set(planId, plan.workspaceId);
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
    if (isCleanedSession(event.payload.workspaceId)) return;
    planWorkspaces.set(event.payload.planId, event.payload.workspaceId);
    void hydrateGraphPlan(event.payload.planId);
  });
  listen<GraphRunEventPayload>("graph-run-event", (event) => {
    applyGraphRunEvent(event.payload);
  });
}

function getGraphPlanSnapshot(planId: string): GraphPlanSnapshot {
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

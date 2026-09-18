/**
 * 断连运行对账。
 *
 * run 事件走随每次 invoke 生存的 `Channel`（`dispatcher_send_*_message` 的
 * onEvent），webview 重载后前端无法重订阅仍在运行的会话——store 里没有
 * 它们的运行标记，表现为「界面无运行态、发消息却被后端以已在运行中拒绝」。
 * 本模块在 App 挂载时查询 `dispatcher_active_runs`，对没有本地 run 槽位
 * （即无存活事件通道）的运行中会话补一个「后台运行中」live state；
 * `dispatcher_stop_run` 不依赖 Channel，停止入口照常可用。随后轮询直到
 * 这些会话收尾，复位运行态并拉全量消息对账。
 */

import { invoke } from "@tauri-apps/api/core";
import {
  createIdleLiveSessionState,
  getDispatcherActiveRunId,
  getDispatcherLiveSessionState,
  notifyDispatcherLiveSessionSubscribers,
  setDispatcherLiveSessionState,
} from "../dispatcherSessionStore";
import { reconcileSessionMessages } from "./event-channel";

const DETACHED_RUN_POLL_INTERVAL_MS = 2000;

const DETACHED_RUN_PLACEHOLDER =
  "后台运行中…（页面已重载，无法显示实时进度；可停止或等待完成）";

export interface DetachedRunPlan {
  /** 后端仍在跑、本地既未托管也没有存活事件通道，需补「后台运行中」的会话。 */
  adopt: string[];
  /** 后端已收尾、此前由本模块托管的会话（释放前应再确认无本地新 run 接管）。 */
  release: string[];
}

/** 纯决策核心：比对后端运行集合与本地托管集合，产出收养/释放计划。 */
export function planDetachedRuns(
  backendActive: ReadonlySet<string>,
  tracked: ReadonlySet<string>,
  hasLiveChannel: (sessionId: string) => boolean,
): DetachedRunPlan {
  const adopt: string[] = [];
  for (const sessionId of backendActive) {
    if (tracked.has(sessionId) || hasLiveChannel(sessionId)) continue;
    adopt.push(sessionId);
  }
  const release: string[] = [];
  for (const sessionId of tracked) {
    if (backendActive.has(sessionId)) continue;
    release.push(sessionId);
  }
  return { adopt, release };
}

let reconciliationStarted = false;
let pollTimer: number | null = null;
/** 启动期查询失败的有界重试计数（成功后清零；避免 IPC 异常时无限轮询）。 */
let bootRetryCount = 0;
const BOOT_RETRY_LIMIT = 3;
const trackedDetachedRuns = new Set<string>();

function applyDetachedRunState(sessionId: string, active: boolean) {
  const current = getDispatcherLiveSessionState(sessionId) ?? createIdleLiveSessionState();
  const next = {
    ...current,
    hasPendingRun: active,
    isLoading: active,
    assistantPlaceholder: active ? DETACHED_RUN_PLACEHOLDER : null,
  };
  setDispatcherLiveSessionState(sessionId, next);
  notifyDispatcherLiveSessionSubscribers(sessionId, next);
}

async function pollDetachedRuns(): Promise<void> {
  let backendActive: ReadonlySet<string>;
  try {
    backendActive = new Set(await invoke<string[]>("dispatcher_active_runs"));
  } catch (error) {
    console.error("查询运行中会话失败:", error);
    // ensure 只在 App 挂载时触发一次，启动期查询失败需要就地有界重试，
    // 否则整个应用会话都会错过对账。
    if (trackedDetachedRuns.size === 0 && bootRetryCount < BOOT_RETRY_LIMIT) {
      bootRetryCount += 1;
      pollTimer = window.setTimeout(() => {
        pollTimer = null;
        void pollDetachedRuns();
      }, DETACHED_RUN_POLL_INTERVAL_MS);
      return;
    }
    scheduleNextPoll();
    return;
  }
  bootRetryCount = 0;
  const plan = planDetachedRuns(backendActive, trackedDetachedRuns, (sessionId) =>
    getDispatcherActiveRunId(sessionId) !== undefined,
  );
  for (const sessionId of plan.adopt) {
    trackedDetachedRuns.add(sessionId);
    applyDetachedRunState(sessionId, true);
  }
  for (const sessionId of plan.release) {
    trackedDetachedRuns.delete(sessionId);
    // 释放瞬间若本地已开启新 run（用户在停止后立刻重发），事件通道已接管
    // 该会话状态，不覆盖、不对账（对账的竞态守卫也会让位给活动 run）。
    if (getDispatcherActiveRunId(sessionId) !== undefined) continue;
    applyDetachedRunState(sessionId, false);
    reconcileSessionMessages(sessionId);
  }
  scheduleNextPoll();
}

function scheduleNextPoll() {
  // 托管集清空即停轮询并允许下次 ensure 重新启动：断连会话只会因 webview
  // 重载产生（模块状态随之重置），存活期间的新 run 都有自己的事件通道。
  // 查询失败且无托管会话时同样停表——下一次挂载（如切换视图触发 hook）
  // 会经 ensure 重试。
  if (trackedDetachedRuns.size === 0) {
    reconciliationStarted = false;
    if (pollTimer !== null) {
      clearTimeout(pollTimer);
      pollTimer = null;
    }
    return;
  }
  if (pollTimer !== null) return;
  pollTimer = window.setTimeout(() => {
    pollTimer = null;
    void pollDetachedRuns();
  }, DETACHED_RUN_POLL_INTERVAL_MS);
}

/** 幂等启动断连对账（App 挂载时调用一次；重载后模块状态重置自然重启）。 */
export function ensureRunStateReconciliation() {
  if (reconciliationStarted) return;
  reconciliationStarted = true;
  void pollDetachedRuns();
}

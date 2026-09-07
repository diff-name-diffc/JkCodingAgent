import { listen } from "@tauri-apps/api/event";

/**
 * shell-output-bus.ts —— `shell-output` 事件的全应用单一监听者（UI-24 遗留⑧）。
 *
 * 每个保活终端原先各自 `listen("shell-output")`，N 个保活终端 = N 个全局监听者，
 * 每个都收到所有终端的每一次输出事件（内部再按 shell_id 过滤）。这里收敛为
 * 一个模块级懒注册监听者，按 `shell_id` 分发给对应终端的回调集合；退订后集合
 * 清空则删除键。与 `subAgentEventStore.ts` 的 `registerGlobalListener` 单例守卫、
 * `graph-store.ts` 的既有单例模式保持一致。
 */

interface ShellOutputPayload {
  shell_id: string;
  data: string;
}

type ShellOutputCallback = (data: string) => void;

const subscribers = new Map<string, Set<ShellOutputCallback>>();
let listenerRegistered = false;

function registerGlobalListener(): void {
  if (listenerRegistered) return;
  listenerRegistered = true;

  listen<ShellOutputPayload>("shell-output", (event) => {
    const callbacks = subscribers.get(event.payload.shell_id);
    if (!callbacks) return;
    // 快照迭代：回调内退订不破坏当前分发。
    for (const cb of Array.from(callbacks)) cb(event.payload.data);
  });
}

/**
 * 订阅指定终端（shellId）的输出。返回退订函数；当该终端无订阅者时删除其键。
 */
export function subscribeShellOutput(
  shellId: string,
  callback: ShellOutputCallback,
): () => void {
  registerGlobalListener();

  let callbacks = subscribers.get(shellId);
  if (!callbacks) {
    callbacks = new Set();
    subscribers.set(shellId, callbacks);
  }
  callbacks.add(callback);

  return () => {
    const set = subscribers.get(shellId);
    if (!set) return;
    set.delete(callback);
    if (set.size === 0) subscribers.delete(shellId);
  };
}

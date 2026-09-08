import type { SaveSourceMode } from "./save-status";

/**
 * 设置各页保存状态的统一注册表（UI-21 遗留领取）。
 *
 * 头部 SaveStatusIndicator 此前只反映 use-aha-settings 全局管线的状态；
 * SSH 服务器页与 MCP 页各自持有 400ms debounce 本地保存管线、RAG 配置页为
 * 手动保存，三者的保存进度不进入指示器——在 SSH 页保存失败时头部可能仍显示
 * 「已保存」。本注册表让各页把 {dirty/saving/hasError/mode} 快照发布到模块级
 * 单例（模式参照 use-aha-settings / graph-store），由指示器与关闭脏检查聚合消费。
 *
 * 生命周期：页面挂载时 `registerSaveSource(id, flush)`、卸载时注销（返回的
 * 清理函数）；状态变化时 `publishSaveSource(id, status)` 推送快照。快照同值
 * 重复发布不通知（订阅者免无谓重渲染）。StrictMode 双挂载下同 id 二次注册会
 * 覆盖首个实例，首个清理函数凭 flush 引用识别「已被覆盖」而不误删后来者。
 *
 * 卸载语义：SSH/MCP 页卸载（切导航页）前自行 flush-then-clear（修复 400ms
 * 窗口内切页丢编辑的既有缺陷，见各自页面）；RAG 为 manual 模式——切页放弃
 * 未保存修改是其既有语义（与手动保存模型一致），仅关闭弹窗时经确认框
 * 「保存并关闭」走 flushAllSaveSources。
 */

export interface SaveSourceStatus {
  mode: SaveSourceMode;
  /** auto：有等待中的 debounce 或正在保存；manual：有未保存的修改。 */
  dirty: boolean;
  saving: boolean;
  hasError: boolean;
}

export interface RegisteredSaveSource extends SaveSourceStatus {
  /** 立即落盘（清 debounce timer 后保存）；错误由源自身 toast/内联呈现。 */
  flush: () => Promise<void>;
}

type Subscriber = () => void;

const sources = new Map<string, RegisteredSaveSource>();
const subscribers = new Set<Subscriber>();

/** 对外快照：数组身份仅在注册/注销/状态变化时重建，供 useSyncExternalStore 比较。 */
let snapshot: RegisteredSaveSource[] = [];

function rebuildSnapshot(): void {
  snapshot = [...sources.values()];
}

function notify(): void {
  rebuildSnapshot();
  for (const subscriber of subscribers) subscriber();
}

function sameStatus(a: SaveSourceStatus, b: SaveSourceStatus): boolean {
  return (
    a.mode === b.mode && a.dirty === b.dirty && a.saving === b.saving && a.hasError === b.hasError
  );
}

/** 注册一个保存源；返回注销函数（页面卸载时调用）。 */
export function registerSaveSource(id: string, flush: () => Promise<void>): () => void {
  sources.set(id, { mode: "auto", dirty: false, saving: false, hasError: false, flush });
  notify();
  return () => {
    // 同 id 已被新实例覆盖时不误删后来者（StrictMode 双挂载）。
    if (sources.get(id)?.flush === flush) sources.delete(id);
    notify();
  };
}

/** 推送保存源最新快照；未注册（或与当前快照同值）时为 no-op。 */
export function publishSaveSource(id: string, status: SaveSourceStatus): void {
  const source = sources.get(id);
  if (!source || sameStatus(source, status)) return;
  sources.set(id, { ...source, ...status });
  notify();
}

export function subscribeSaveSources(subscriber: Subscriber): () => void {
  subscribers.add(subscriber);
  return () => {
    subscribers.delete(subscriber);
  };
}

export function getSaveSourcesSnapshot(): RegisteredSaveSource[] {
  return snapshot;
}

/** 关闭弹窗的脏检查：任一注册源仍在保存中/有未保存修改。 */
export function hasDirtySaveSources(): boolean {
  return snapshot.some((source) => source.dirty || source.saving);
}

/** 并行 flush 所有 dirty 源（关闭前落盘）；saving 中的源由自身管线收敛。 */
export async function flushAllSaveSources(): Promise<void> {
  await Promise.all(
    getSaveSourcesSnapshot()
      .filter((source) => source.dirty)
      .map((source) => source.flush()),
  );
}

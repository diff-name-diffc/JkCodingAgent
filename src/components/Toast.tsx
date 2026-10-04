import { useSyncExternalStore } from "react";
import type React from "react";

export type ToastKind = "success" | "error" | "warning";

/** 同屏 toast 上限，超出丢最旧。 */
const TOAST_CAP = 4;

export type ToastItem = {
  id: number;
  kind: ToastKind;
  message: string;
};

let toasts: ToastItem[] = [];
let nextId = 1;
const listeners = new Set<() => void>();
const timers = new Map<number, ReturnType<typeof setTimeout>>();

/** success 是即时正向确认，比错误/警告更快消失。 */
export function toastDurationMs(kind: ToastKind): number {
  return kind === "success" ? 2500 : 5000;
}

/**
 * 相同 kind+文案的旧条目先移除再追加（重复提示合并并重置时长，
 * 避免自动保存等高频场景堆叠），随后按上限截断丢最旧。
 */
export function mergeToastList(prev: ToastItem[], next: ToastItem, cap = TOAST_CAP): ToastItem[] {
  return [...prev.filter((t) => !(t.kind === next.kind && t.message === next.message)), next].slice(
    -cap,
  );
}

function emit() {
  listeners.forEach((listener) => listener());
}

function dismiss(id: number) {
  const timer = timers.get(id);
  if (timer) {
    clearTimeout(timer);
    timers.delete(id);
  }
  if (toasts.some((t) => t.id === id)) {
    toasts = toasts.filter((t) => t.id !== id);
    emit();
  }
}

function push(kind: ToastKind, message: string) {
  const id = nextId++;
  toasts = mergeToastList(toasts, { id, kind, message });
  emit();
  timers.set(
    id,
    setTimeout(() => dismiss(id), toastDurationMs(kind)),
  );
}

/** 全局命令式 toast：组件、hooks 与模块级 store（非渲染路径）均可直接调用。 */
export const toast = {
  success: (message: string) => push("success", message),
  error: (message: string) => push("error", message),
  warning: (message: string) => push("warning", message),
  dismiss,
};

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function getSnapshot(): ToastItem[] {
  return toasts;
}

function useToasts(): ToastItem[] {
  return useSyncExternalStore(subscribe, getSnapshot);
}

/** 应用级 toast 视口的唯一挂载点（main.tsx）：children 之后渲染全局右下角提示栈。 */
export function ToastProvider({ children }: { children: React.ReactNode }) {
  const items = useToasts();
  if (items.length === 0) return <>{children}</>;
  return (
    <>
      {children}
      <div className="ai-toast-stack" role="status" aria-live="polite">
        {items.map((t) => (
          <div key={t.id} className={`ai-toast-item is-${t.kind}`}>
            <span className="ai-toast-message">{t.message}</span>
            <button onClick={() => toast.dismiss(t.id)} className="ai-toast-close">
              ×
            </button>
          </div>
        ))}
      </div>
    </>
  );
}

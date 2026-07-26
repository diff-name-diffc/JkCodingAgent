import { useSyncExternalStore } from "react";

export type ToastKind = "success" | "error";

export type ToastItem = {
  id: number;
  kind: ToastKind;
  message: string;
};

let toasts: ToastItem[] = [];
let nextId = 1;
const listeners = new Set<() => void>();
const timers = new Map<number, ReturnType<typeof setTimeout>>();

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
  // 相同文案的连续 toast 合并为一个，避免自动保存高频触发时堆叠。
  toasts = [...toasts.filter((t) => !(t.kind === kind && t.message === message)), { id, kind, message }].slice(-4);
  emit();
  timers.set(
    id,
    setTimeout(() => dismiss(id), kind === "error" ? 5000 : 2500),
  );
}

export const toast = {
  success: (message: string) => push("success", message),
  error: (message: string) => push("error", message),
  dismiss,
};

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useToasts(): ToastItem[] {
  return useSyncExternalStore(subscribe, () => toasts);
}

import { createContext, useContext, useState, useCallback, useRef } from "react";
import type React from "react";

interface ToastItem {
  id: string;
  message: string;
  type: "error" | "warning";
}

interface ToastContextValue {
  showToast: (message: string, type?: "error" | "warning") => void;
}

const ToastContext = createContext<ToastContextValue>({ showToast: () => {} });

export function useToast() {
  return useContext(ToastContext);
}

export function ToastProvider({ children }: { children: React.ReactNode }) {
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const timerMap = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map());

  const showToast = useCallback((message: string, type: "error" | "warning" = "error") => {
    const id = `${Date.now()}-${Math.random()}`;
    setToasts((prev) => [...prev.slice(-2), { id, message, type }]);
    const timer = setTimeout(() => {
      setToasts((prev) => prev.filter((t) => t.id !== id));
      timerMap.current.delete(id);
    }, 5000);
    timerMap.current.set(id, timer);
  }, []);

  const dismiss = useCallback((id: string) => {
    const timer = timerMap.current.get(id);
    if (timer) {
      clearTimeout(timer);
      timerMap.current.delete(id);
    }
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  return (
    <ToastContext.Provider value={{ showToast }}>
      {children}
      <ToastContainer toasts={toasts} onDismiss={dismiss} />
    </ToastContext.Provider>
  );
}

function ToastContainer({
  toasts,
  onDismiss,
}: {
  toasts: ToastItem[];
  onDismiss: (id: string) => void;
}) {
  if (toasts.length === 0) return null;
  return (
    <div className="ai-toast-stack ai-migrated-toast-stack">
      {toasts.map((t) => (
        <div
          key={t.id}
          className={t.type === "error" ? "ai-toast-item is-error" : "ai-toast-item is-warning"}
        >
          <span className="ai-toast-message">{t.message}</span>
          <button
            onClick={() => onDismiss(t.id)}
            className="ai-toast-close"
          >
            ×
          </button>
        </div>
      ))}
    </div>
  );
}

import { Check, X } from "lucide-react";
import { toast, useToasts } from "./toast";

/** 右上角轻量 toast 容器，挂载在设置弹窗内（z-index 高于弹窗）。 */
export function Toaster() {
  const toasts = useToasts();
  if (toasts.length === 0) return null;
  return (
    <div className="ai-set-toaster" role="status" aria-live="polite">
      {toasts.map((item) => (
        <div
          key={item.id}
          className={
            item.kind === "success" ? "ai-set-toast is-success" : "ai-set-toast is-error"
          }
        >
          {item.kind === "success" ? (
            <Check size={16} strokeWidth={1.5} />
          ) : (
            <X size={16} strokeWidth={1.5} />
          )}
          <span className="ai-set-toast-message">{item.message}</span>
          <button
            type="button"
            className="ai-set-toast-close"
            onClick={() => toast.dismiss(item.id)}
            aria-label="关闭提示"
          >
            <X size={14} strokeWidth={1.5} />
          </button>
        </div>
      ))}
    </div>
  );
}

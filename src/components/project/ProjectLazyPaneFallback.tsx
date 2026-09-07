import { Loader2 } from "lucide-react";

/**
 * 懒加载面板的首次加载占位（UI-25）。
 *
 * spinner + 文案双编码，明确表达「加载中」——与空态（无数据）、错误
 * （外层 ErrorBoundary 承接并带重试）在视觉上区分开，不把三者混为一谈。
 */
export function ProjectLazyPaneFallback({ label = "加载中..." }: { label?: string }) {
  return (
    <div
      role="status"
      aria-live="polite"
      style={{
        flex: 1,
        minWidth: 0,
        minHeight: 0,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        gap: 8,
        color: "var(--text-muted)",
        fontSize: 13,
        background: "var(--bg-panel)",
      }}
    >
      <Loader2 className="animate-spin" size={16} strokeWidth={2} aria-hidden="true" />
      <span>{label}</span>
    </div>
  );
}

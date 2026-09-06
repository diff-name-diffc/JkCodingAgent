import { AlertTriangle, Check, Clock3, Loader2, Minus, X, type LucideIcon } from "lucide-react";
import { cn } from "../../lib/cn";
import { resolveStatusMeta, type StatusDomain, type StatusTone } from "./status-meta";

/**
 * 状态双编码 pill（图标形状 + 文字，配色走 tone）——tokens.md §5 评审结论 4
 * 的统一落点：工具卡、聚合摘要行、执行图、子智能体、Python 状态共用。
 */
const TONE_ICON: Record<StatusTone, LucideIcon> = {
  success: Check,
  error: X,
  running: Loader2,
  warn: AlertTriangle,
  neutral: Minus,
  pending: Clock3,
};

export interface StatusPillProps {
  domain: StatusDomain;
  status: string;
  /** 覆盖默认 label（如聚合计数「失败 2」）；tone/图标仍由 status 决定。 */
  label?: string;
  className?: string;
}

export function StatusPill({ domain, status, label, className }: StatusPillProps) {
  const meta = resolveStatusMeta(domain, status);
  const Icon = TONE_ICON[meta.tone];
  return (
    <span className={cn("ai-status-pill", `ai-status-pill--${meta.tone}`, className)}>
      <Icon
        size={11}
        strokeWidth={2.2}
        className={meta.tone === "running" ? "animate-spin" : undefined}
        aria-hidden="true"
      />
      {label ?? meta.label}
    </span>
  );
}

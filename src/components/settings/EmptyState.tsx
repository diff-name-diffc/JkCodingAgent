import type { LucideIcon } from "lucide-react";

/** 空状态：图标 + 一句引导文案 + 主按钮。 */
export function EmptyState({
  icon: Icon,
  title,
  actionLabel,
  onAction,
}: {
  icon: LucideIcon;
  title: string;
  actionLabel?: string;
  onAction?: () => void;
}) {
  return (
    <div className="ai-set-empty">
      <div className="ai-set-empty-icon">
        <Icon size={20} strokeWidth={1.5} />
      </div>
      <p className="ai-set-empty-title">{title}</p>
      {actionLabel && onAction && (
        <button type="button" className="ai-primary-button" onClick={onAction}>
          {actionLabel}
        </button>
      )}
    </div>
  );
}

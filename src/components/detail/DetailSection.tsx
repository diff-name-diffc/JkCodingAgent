import type * as React from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import { cn } from "../../lib/cn";

export interface DetailSectionProps {
  title: React.ReactNode;
  /** 标题右侧的次级说明（字符数、状态提示等）。 */
  hint?: React.ReactNode;
  /** 标题行右端动作区。 */
  actions?: React.ReactNode;
  /** 传入后标题行成为折叠切换按钮（open 受控）。 */
  onToggle?: () => void;
  open?: boolean;
  className?: string;
  children?: React.ReactNode;
}

/**
 * 详情视图通用分区（UI-14）：子智能体、节点抽屉、产物与 Python 详情
 * 共用同一分区标题/提示/动作结构，替代三套自有 section 视觉。
 */
export function DetailSection({
  title,
  hint,
  actions,
  onToggle,
  open,
  className,
  children,
}: DetailSectionProps) {
  const headContent = (
    <>
      {onToggle &&
        (open ? (
          <ChevronDown className="ai-detail-section-chevron" aria-hidden />
        ) : (
          <ChevronRight className="ai-detail-section-chevron" aria-hidden />
        ))}
      <span className="ai-detail-section-title">{title}</span>
      {hint != null && <span className="ai-detail-section-hint">{hint}</span>}
      {actions != null && <span className="ai-detail-section-actions">{actions}</span>}
    </>
  );
  return (
    <section className={cn("ai-detail-section", className)}>
      {onToggle ? (
        <button
          type="button"
          className="ai-detail-section-toggle"
          onClick={onToggle}
          aria-expanded={open}
        >
          {headContent}
        </button>
      ) : (
        <div className="ai-detail-section-head">{headContent}</div>
      )}
      {children}
    </section>
  );
}

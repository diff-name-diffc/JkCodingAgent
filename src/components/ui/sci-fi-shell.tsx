import type React from "react";
import { cn } from "../../lib/cn";

export function AiPanel({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("ai-panel", className)} {...props} />;
}

export function AiSectionHeader({
  title,
  caption,
  action,
  className,
}: {
  title: React.ReactNode;
  caption?: React.ReactNode;
  action?: React.ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("ai-section-header", className)}>
      <div className="min-w-0">
        <div className="ai-section-title">{title}</div>
        {caption ? <div className="ai-section-caption">{caption}</div> : null}
      </div>
      {action ? <div className="ai-section-action">{action}</div> : null}
    </div>
  );
}

export function AiStatusPill({
  tone = "neutral",
  className,
  ...props
}: React.HTMLAttributes<HTMLSpanElement> & {
  tone?: "neutral" | "accent" | "success" | "warning" | "danger";
}) {
  return <span className={cn("ai-status-pill", `ai-status-pill--${tone}`, className)} {...props} />;
}

export function AiEmptyState({
  icon,
  title,
  description,
  action,
  className,
}: {
  icon?: React.ReactNode;
  title: React.ReactNode;
  description?: React.ReactNode;
  action?: React.ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("ai-empty-state", className)}>
      {icon ? <div className="ai-empty-icon">{icon}</div> : null}
      <div className="ai-empty-title">{title}</div>
      {description ? <div className="ai-empty-description">{description}</div> : null}
      {action ? <div className="ai-empty-action">{action}</div> : null}
    </div>
  );
}

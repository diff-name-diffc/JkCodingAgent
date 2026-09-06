import type * as React from "react";
import { cn } from "../../lib/cn";

export type ActivityRowStatus = "running" | "success" | "error" | "neutral";

export interface ActivityRow {
  id: string;
  /** 主标签（工具名/步骤名，可含图标）。 */
  label: React.ReactNode;
  /** 状态文字/耗时等右侧元信息。 */
  meta?: React.ReactNode;
  /** 次级说明（参数预览等）。 */
  detail?: React.ReactNode;
  status: ActivityRowStatus;
  /** 展开内容（结果预览、审查记录等）。 */
  children?: React.ReactNode;
}

/**
 * 简单活动时间线（UI-14 共享）：竖线 + 状态点 + 行内容。
 * 面向子智能体/Python 步骤这类十级~百级行数的列表；千级行数的
 * 图节点活动继续使用虚拟化 ExecutionTimelineList（量级决策，
 * 状态视觉经 status-meta/StatusPill 统一）。
 */
export function ActivityTimeline({ rows }: { rows: ActivityRow[] }) {
  if (rows.length === 0) return null;
  return (
    <div className="ai-activity-timeline">
      {rows.map((row, index) => (
        <div key={row.id} className="ai-activity-timeline-item">
          {index < rows.length - 1 && <span className="ai-activity-timeline-line" aria-hidden />}
          <span
            className={cn("ai-activity-timeline-dot", `ai-activity-timeline-dot--${row.status}`)}
            aria-hidden
          />
          <div className="ai-activity-timeline-content">
            <div className="ai-activity-timeline-head">
              <span className="ai-activity-timeline-label">{row.label}</span>
              {row.meta != null && <span className="ai-activity-timeline-meta">{row.meta}</span>}
            </div>
            {row.detail != null && <div className="ai-activity-timeline-detail">{row.detail}</div>}
            {row.children}
          </div>
        </div>
      ))}
    </div>
  );
}

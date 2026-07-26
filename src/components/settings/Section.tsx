import type { ReactNode } from "react";

/** 页内分区：标题 + 描述 + 内容，分区间距 24px 由 .ai-set-section 样式保证。 */
export function Section({
  id,
  title,
  description,
  aside,
  children,
}: {
  id?: string;
  title: string;
  description?: string;
  /** 标题行尾部的辅助内容（如条目数徽标）。 */
  aside?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section id={id} className="ai-set-section">
      <div className="ai-set-section-head">
        <h3 className="ai-set-section-title">
          {title}
          {aside}
        </h3>
        {description && <p className="ai-set-section-description">{description}</p>}
      </div>
      {children}
    </section>
  );
}

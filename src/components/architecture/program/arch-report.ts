/**
 * 画布程序执行报告构造（≤950 字符硬上限，保住末尾的截图引用）。
 *
 * 从 `arch-program.ts` 拆出：DSL 类型/校验与报告构造变化原因不同。
 */

export const ARCH_REPORT_MAX_CHARS = 950;

export interface ArchRunStats {
  total: number;
  created: number;
  updated: number;
  moved: number;
  deleted: number;
  arrows: number;
  layouts: number;
  reparented: number;
  /** select_shapes + camera 合计（视图类操作）。 */
  views: number;
}

/** 成功报告：统计 + ref→shapeId 映射（超长截断）+ 截图引用。 */
export function buildArchSuccessReport(
  stats: ArchRunStats,
  refMap: ReadonlyMap<string, string>,
  totalShapes: number,
  screenshotImageId: string | null,
): string {
  const parts: string[] = [];
  const actions: string[] = [];
  if (stats.created > 0) actions.push(`创建 ${stats.created}`);
  if (stats.arrows > 0) actions.push(`箭头 ${stats.arrows}`);
  if (stats.updated > 0) actions.push(`更新 ${stats.updated}`);
  if (stats.moved > 0) actions.push(`移动 ${stats.moved}`);
  if (stats.deleted > 0) actions.push(`删除 ${stats.deleted}`);
  if (stats.layouts > 0) actions.push(`布局 ${stats.layouts}`);
  if (stats.reparented > 0) actions.push(`容器 ${stats.reparented}`);
  if (stats.views > 0) actions.push(`视图 ${stats.views}`);
  parts.push(`画布程序执行成功：${stats.total} 条指令（${actions.join(" / ")}）。`);

  if (refMap.size > 0) {
    const entries = [...refMap.entries()].map(([ref, id]) => `${ref}→${id}`);
    let mapping = `创建映射：${entries.join("，")}`;
    if (mapping.length > 500) {
      mapping = `创建映射（部分）：${entries.slice(0, 8).join("，")}…等 ${entries.length} 个`;
    }
    parts.push(mapping);
  }
  parts.push(`画布现有 ${totalShapes} 个形状。`);
  if (screenshotImageId) {
    parts.push(`执行区域截图：chat-image://${screenshotImageId}`);
  }
  return truncateArchReport(parts.join("\n"));
}

/** 失败报告：指明失败指令与原因；画布已整体回滚。 */
export function buildArchFailureReport(
  failedIndex: number,
  failedType: string,
  reason: string,
): string {
  return truncateArchReport(
    `错误：第 ${failedIndex + 1} 条指令（${failedType}）失败：${reason}。已整体回滚，画布无变化。`,
  );
}

/** 硬截断：保留开头（结论/错误原因）；截图引用若存在则尽力保留在末尾。 */
export function truncateArchReport(report: string): string {
  if ([...report].length <= ARCH_REPORT_MAX_CHARS) return report;
  const imageMatch = report.match(/chat-image:\/\/[0-9A-Za-z-]{8,64}/);
  const suffix = imageMatch ? `\n${imageMatch[0]}` : "";
  const budget = ARCH_REPORT_MAX_CHARS - suffix.length - 1;
  const chars = [...report];
  return `${chars.slice(0, Math.max(0, budget)).join("")}…${suffix}`;
}

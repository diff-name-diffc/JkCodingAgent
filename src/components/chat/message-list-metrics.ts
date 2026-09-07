/**
 * message-list-metrics.ts — 消息列表窗口化参数与判定纯函数（UI-24）。
 *
 * 24a-3 时代这里承载「固定 180px 估高 + spacer」的区间推算；24b-3 起
 * 窗口化本体迁移到 @tanstack/react-virtual 动态测量（measureElement），
 * 区间/占位由 virtualizer 内部维护，本模块只保留：
 * - 开窗阈值判定（>300 条才开窗——小列表全量渲染，避免虚拟化开销与
 *   行卸载语义，300/301 边界为验收口径）；
 * - virtualizer 的初始估高与 overscan 常量（估高只影响首帧布局与未测量
 *   行的临时占位，滚动中由实测值逐行修正——这正是 A11「跳读/大片空白」
 *   的修复点）。
 */

/** 初始行估高（px）——仅作 react-virtual estimateSize 初值，实测后修正。 */
export const ROW_ESTIMATE_PX = 180;
/** 窗口上下各多渲染的行数。 */
export const OVERSCAN_ROWS = 8;
/** 超过该条数才启用窗口化（300/301 边界）。 */
export const WINDOWING_THRESHOLD = 300;

/** 是否启用窗口化：itemCount 严格大于阈值（300 不开窗、301 开窗）。 */
export function shouldUseWindowing(
  itemCount: number,
  threshold: number = WINDOWING_THRESHOLD,
): boolean {
  return itemCount > threshold;
}

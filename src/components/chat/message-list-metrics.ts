/**
 * message-list-metrics.ts — 消息列表窗口化索引计算纯函数（UI-24a-3）。
 *
 * 从 message-list.tsx 平移的既有语义（不改行为）：超过阈值才开窗，
 * 固定估高 + overscan 推算可见区间。node 可测。
 *
 * 边界声明：rowEstimate=180 固定估高与 >300 条阈值是审计 A11 登记的
 * 「跳读/大片空白」根因，属 UI-24b 的动态测量改造范围（react-virtual
 * measureElement）——本模块只承载现状语义，24b 将整体替换。
 */

/** 固定行估高（px）——UI-24b 待替换为动态测量。 */
export const ROW_ESTIMATE_PX = 180;
/** 窗口上下各多渲染的行数。 */
export const OVERSCAN_ROWS = 8;
/** 超过该条数才启用窗口化（300/301 边界）。 */
export const WINDOWING_THRESHOLD = 300;

export interface WindowRangeOptions {
  itemCount: number;
  scrollTop: number;
  viewportHeight: number;
  rowEstimate?: number;
  overscan?: number;
  windowingThreshold?: number;
}

export interface WindowRange {
  useWindowing: boolean;
  startIndex: number;
  endIndex: number;
}

/** 由滚动位置推算可见区间：未开窗时全量渲染（0..itemCount）。 */
export function computeWindowRange(opts: WindowRangeOptions): WindowRange {
  const rowEstimate = opts.rowEstimate ?? ROW_ESTIMATE_PX;
  const overscan = opts.overscan ?? OVERSCAN_ROWS;
  const threshold = opts.windowingThreshold ?? WINDOWING_THRESHOLD;
  const useWindowing = opts.itemCount > threshold;
  if (!useWindowing) {
    return { useWindowing, startIndex: 0, endIndex: opts.itemCount };
  }
  const scrollTop = Math.max(0, opts.scrollTop);
  const viewportHeight = Math.max(0, opts.viewportHeight);
  const startIndex = Math.max(0, Math.floor(scrollTop / rowEstimate) - overscan);
  const endIndex = Math.min(
    opts.itemCount,
    Math.ceil((scrollTop + viewportHeight) / rowEstimate) + overscan,
  );
  return { useWindowing, startIndex, endIndex };
}

/** 上/下占位高度（px）：与 computeWindowRange 同一估高口径。 */
export function windowSpacerHeights(
  range: WindowRange,
  itemCount: number,
  rowEstimate = ROW_ESTIMATE_PX,
): { top: number; bottom: number } {
  if (!range.useWindowing) return { top: 0, bottom: 0 };
  return {
    top: range.startIndex * rowEstimate,
    bottom: Math.max(0, (itemCount - range.endIndex) * rowEstimate),
  };
}

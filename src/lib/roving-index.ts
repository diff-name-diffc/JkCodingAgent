/**
 * roving-index.ts — 列表/标签条方向键导航的索引计算纯函数（UI-23d）。
 *
 * 覆盖 tablist 方向键（ContextNav 四页签）与列表 ↑↓ 导航（会话列表/命令
 * 面板）的公共数学：方向键 ±1、Home/End 跳首尾、wrap 环绕或边界钳制。
 * DOM 焦点移动在 lib/list-focus 薄层；本模块 node 可测。
 */

export type RovingOrientation = "horizontal" | "vertical" | "both";

export interface NextRovingIndexOptions {
  /** 项目总数；<=0 时恒返回 0。 */
  count: number;
  /** 当前索引；越界（含 -1 = 焦点在列表外）先钳制到 [0, count-1]。 */
  current: number;
  key: string;
  /** true（默认）首尾环绕；false 边界钳制。 */
  wrap?: boolean;
  /** 参与判定的方向键轴向：horizontal=←→、vertical=↑↓、both=四向（默认）。 */
  orientation?: RovingOrientation;
}

const PREV_KEYS: Record<RovingOrientation, string[]> = {
  horizontal: ["ArrowLeft"],
  vertical: ["ArrowUp"],
  both: ["ArrowLeft", "ArrowUp"],
};
const NEXT_KEYS: Record<RovingOrientation, string[]> = {
  horizontal: ["ArrowRight"],
  vertical: ["ArrowDown"],
  both: ["ArrowRight", "ArrowDown"],
};

/** 是否 roving 导航键（调用方据此决定 preventDefault）。 */
export function isRovingKey(key: string, orientation: RovingOrientation = "both"): boolean {
  return (
    PREV_KEYS[orientation].includes(key) ||
    NEXT_KEYS[orientation].includes(key) ||
    key === "Home" ||
    key === "End"
  );
}

/** 计算下一个索引；非导航键原样返回钳制后的 current。 */
export function nextRovingIndex(opts: NextRovingIndexOptions): number {
  if (opts.count <= 0) return 0;
  const orientation = opts.orientation ?? "both";
  const current = Math.max(0, Math.min(opts.count - 1, opts.current));
  if (opts.count === 1) return 0;
  if (opts.key === "Home") return 0;
  if (opts.key === "End") return opts.count - 1;
  let delta: number;
  if (PREV_KEYS[orientation].includes(opts.key)) delta = -1;
  else if (NEXT_KEYS[orientation].includes(opts.key)) delta = 1;
  else return current;
  const next = current + delta;
  if (opts.wrap === false) return Math.max(0, Math.min(opts.count - 1, next));
  return ((next % opts.count) + opts.count) % opts.count;
}

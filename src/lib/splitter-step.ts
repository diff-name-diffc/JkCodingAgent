/**
 * splitter-step.ts — 分隔条键盘步进纯函数（UI-23c）。
 *
 * 统一 5 条工作区分隔条的方向键语义：
 *   - ratio 型（0..1 占比，如会话↔编辑区 splitter）：小步 0.02、Shift 大步 0.1
 *     ——与 ProjectWorkbenchContent 既有实现完全一致；
 *   - px 型（导航/侧栏宽度、终端高度、浏览器面板宽度）：小步 8px、Shift 大步 48px；
 *   - 双击复位到各站点默认值（resetSplitterValue 统一钳制）。
 *
 * 方向映射：垂直分隔条（左右分栏）用 ArrowLeft/Right，水平分隔条（上下分栏）
 * 用 ArrowUp/Down；「正向键增大值」，invert=true 的站点（值增大方向与正向键
 * 相反，如从右缘测量的编辑区占比、右停靠面板）翻转映射。
 */

export type SplitterOrientation = "vertical" | "horizontal";
export type SplitterMode = "ratio" | "px";

export const SPLITTER_RATIO_STEP = 0.02;
export const SPLITTER_RATIO_LARGE_STEP = 0.1;
export const SPLITTER_PX_STEP = 8;
export const SPLITTER_PX_LARGE_STEP = 48;

/**
 * 方向键 → 值增量。正向键（垂直=ArrowRight / 水平=ArrowUp）为 +1，
 * 反向键为 -1，invert 翻转；非方向键返回 null（调用方不拦截）。
 */
export function splitterKeyDelta(
  key: string,
  orientation: SplitterOrientation,
  invert = false,
): 1 | -1 | null {
  const forward = orientation === "vertical" ? "ArrowRight" : "ArrowUp";
  const backward = orientation === "vertical" ? "ArrowLeft" : "ArrowDown";
  if (key === forward) return invert ? -1 : 1;
  if (key === backward) return invert ? 1 : -1;
  return null;
}

export interface NextSplitterValueOptions {
  current: number;
  /** 值增量方向（splitterKeyDelta 的输出）。 */
  delta: 1 | -1;
  mode: SplitterMode;
  /** Shift 大步。 */
  shift?: boolean;
  min: number;
  max: number;
}

/** 步进后的新值：按 mode 取步长，钳制在 [min, max]，消除浮点误差累积。 */
export function nextSplitterValue(opts: NextSplitterValueOptions): number {
  const step =
    opts.mode === "ratio"
      ? opts.shift
        ? SPLITTER_RATIO_LARGE_STEP
        : SPLITTER_RATIO_STEP
      : opts.shift
        ? SPLITTER_PX_LARGE_STEP
        : SPLITTER_PX_STEP;
  const raw = opts.current + opts.delta * step;
  // ratio 保留两位小数（0.02 步进不产生 0.5000000001）；px 取整。
  const next = opts.mode === "ratio" ? Math.round(raw * 100) / 100 : Math.round(raw);
  if (!Number.isFinite(next)) return opts.current;
  return Math.max(opts.min, Math.min(opts.max, next));
}

/** 双击复位：默认值统一钳制到边界（默认值越界时不产生非法状态）。 */
export function resetSplitterValue(defaultValue: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, defaultValue));
}

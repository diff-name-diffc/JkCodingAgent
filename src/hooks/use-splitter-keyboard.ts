import { useCallback, useRef } from "react";
import {
  nextSplitterValue,
  resetSplitterValue,
  splitterKeyDelta,
  type SplitterMode,
  type SplitterOrientation,
} from "../lib/splitter-step";

export interface SplitterKeyboardConfig {
  /** 分隔条方向：vertical=左右分栏（ArrowLeft/Right），horizontal=上下分栏（ArrowUp/Down）。 */
  orientation: SplitterOrientation;
  /** ratio=0..1 占比（aria 值报百分比），px=像素。 */
  mode: SplitterMode;
  ariaLabel: string;
  /** 值增大方向与正向键相反时置 true（右缘测量的占比 / 右停靠面板）。 */
  invert?: boolean;
  getValue: () => number;
  getBounds: () => { min: number; max: number };
  getDefaultValue: () => number;
  /** 键盘步进为即时提交（按键频率天然低，不引入拖拽期高频持久化）。 */
  onCommit: (next: number) => void;
}

/**
 * 分隔条键盘支持（UI-23c）：返回 role=separator 的完整 ARIA 属性与
 * onKeyDown/onDoubleClick——5 条工作区分隔条统一达到既有
 * ProjectWorkspaceLayout splitter 的水准（Tab 可达、方向键步进、Shift 大步、
 * 双击复位、aria-valuenow 可读）。步进语义在 lib/splitter-step 纯函数层。
 *
 * config 走 ref 存最新值，回调身份稳定；鼠标拖拽路径不受影响。
 */
export function useSplitterKeyboard(config: SplitterKeyboardConfig) {
  const configRef = useRef(config);
  configRef.current = config;

  const onKeyDown = useCallback((event: React.KeyboardEvent) => {
    const c = configRef.current;
    const delta = splitterKeyDelta(event.key, c.orientation, c.invert === true);
    if (delta === null) return;
    event.preventDefault();
    const current = c.getValue();
    const { min, max } = c.getBounds();
    const next = nextSplitterValue({
      current,
      delta,
      mode: c.mode,
      shift: event.shiftKey,
      min,
      max,
    });
    if (next !== current) c.onCommit(next);
  }, []);

  const onDoubleClick = useCallback(() => {
    const c = configRef.current;
    const { min, max } = c.getBounds();
    c.onCommit(resetSplitterValue(c.getDefaultValue(), min, max));
  }, []);

  const value = config.getValue();
  const bounds = config.getBounds();
  const clamp = (v: number) => Math.max(bounds.min, Math.min(bounds.max, v));
  const toAria = (v: number) =>
    config.mode === "ratio" ? Math.round(clamp(v) * 100) : Math.round(clamp(v));

  return {
    role: "separator" as const,
    "aria-orientation": config.orientation,
    "aria-label": config.ariaLabel,
    tabIndex: 0,
    "aria-valuenow": toAria(value),
    "aria-valuemin": toAria(bounds.min),
    "aria-valuemax": toAria(bounds.max),
    onKeyDown,
    onDoubleClick,
  };
}

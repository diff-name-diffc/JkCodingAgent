import { useCallback, useRef, useState } from "react";
import type React from "react";
import { load, save } from "../utils";

const BROWSER_PANEL_MIN_WIDTH = 420;
const BROWSER_PANEL_DEFAULT_RATIO = 0.4;
const BROWSER_PANEL_MAX_RATIO = 0.75;

/** 停靠在主内容右侧的可拖宽面板的尺寸参数（默认值 = 浏览器面板历史值）。 */
export interface DockedPanelMetrics {
  minWidth?: number;
  defaultRatio?: number;
  /** 像素锚定的默认宽（优先于 defaultRatio）——设计给固定区间时用，
   * 避免大视口下比例默认值漂移过宽（UI-15 架构助手 320–400px）。 */
  defaultWidthPx?: number;
  maxRatio?: number;
}

function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value));
}

function viewportWidth(): number {
  return typeof window === "undefined" ? 1280 : window.innerWidth;
}

/** 面板默认宽（纯函数）：像素锚定值优先，否则按视口比例；统一钳制在
 * [minWidth, max(minWidth, viewport × maxRatio)]。 */
export function resolveDockedPanelDefaultWidth(
  viewport: number,
  metrics: DockedPanelMetrics,
): number {
  const minWidth = metrics.minWidth ?? BROWSER_PANEL_MIN_WIDTH;
  const defaultRatio = metrics.defaultRatio ?? BROWSER_PANEL_DEFAULT_RATIO;
  const maxRatio = metrics.maxRatio ?? BROWSER_PANEL_MAX_RATIO;
  const maxWidth = Math.max(minWidth, Math.floor(viewport * maxRatio));
  const raw = metrics.defaultWidthPx ?? Math.round(viewport * defaultRatio);
  return clamp(raw, minWidth, maxWidth);
}

export function useDockedBrowserPanel(storageKey: string, metrics: DockedPanelMetrics = {}) {
  const minWidth = metrics.minWidth ?? BROWSER_PANEL_MIN_WIDTH;
  const maxRatio = metrics.maxRatio ?? BROWSER_PANEL_MAX_RATIO;

  const panelMaxWidth = () => Math.max(minWidth, Math.floor(viewportWidth() * maxRatio));
  const defaultWidth = () => resolveDockedPanelDefaultWidth(viewportWidth(), metrics);
  const loadWidth = () => clamp(load<number>(storageKey, defaultWidth()), minWidth, panelMaxWidth());

  const [width, setWidth] = useState(loadWidth);
  const [expanded, setExpanded] = useState(false);
  const widthRef = useRef(width);
  widthRef.current = width;

  const effectiveWidth = expanded ? panelMaxWidth() : width;

  const toggleExpanded = useCallback(() => {
    setExpanded((value) => !value);
  }, []);

  /** 键盘步进/双击复位的即时提交通道（UI-23c）：钳制 + 退出扩大态 + 持久化。 */
  const commitWidth = useCallback(
    (next: number) => {
      const clamped = clamp(next, minWidth, panelMaxWidth());
      setExpanded(false);
      setWidth(clamped);
      save(storageKey, clamped);
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [storageKey, minWidth, maxRatio],
  );

  /** 当前宽度边界（随视口变化，调用时求值）。 */
  const getWidthBounds = useCallback(
    () => ({ min: minWidth, max: panelMaxWidth() }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [minWidth, maxRatio],
  );

  const getDefaultWidth = useCallback(
    () => resolveDockedPanelDefaultWidth(viewportWidth(), metrics),
    [metrics],
  );

  const handleResizeStart = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();
      setExpanded(false);

      const startX = e.clientX;
      const startWidth = widthRef.current;
      let nextWidth = startWidth;

      const onMouseMove = (ev: MouseEvent) => {
        nextWidth = clamp(startWidth + (startX - ev.clientX), minWidth, panelMaxWidth());
        setWidth(nextWidth);
      };
      const onMouseUp = () => {
        save(storageKey, nextWidth);
        document.removeEventListener("mousemove", onMouseMove);
        document.removeEventListener("mouseup", onMouseUp);
        document.body.style.cursor = "";
        document.body.style.userSelect = "";
      };

      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";
      document.addEventListener("mousemove", onMouseMove);
      document.addEventListener("mouseup", onMouseUp);
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [storageKey, minWidth, maxRatio],
  );

  return {
    effectiveWidth,
    /** 未扩大的用户偏好宽度（键盘步进的基准值，UI-23c）。 */
    width,
    expanded,
    toggleExpanded,
    handleResizeStart,
    commitWidth,
    getWidthBounds,
    getDefaultWidth,
  };
}

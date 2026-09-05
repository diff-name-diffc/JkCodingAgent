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
  maxRatio?: number;
}

function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value));
}

function viewportWidth(): number {
  return typeof window === "undefined" ? 1280 : window.innerWidth;
}

export function useDockedBrowserPanel(storageKey: string, metrics: DockedPanelMetrics = {}) {
  const minWidth = metrics.minWidth ?? BROWSER_PANEL_MIN_WIDTH;
  const defaultRatio = metrics.defaultRatio ?? BROWSER_PANEL_DEFAULT_RATIO;
  const maxRatio = metrics.maxRatio ?? BROWSER_PANEL_MAX_RATIO;

  const panelMaxWidth = () => Math.max(minWidth, Math.floor(viewportWidth() * maxRatio));
  const defaultWidth = () =>
    clamp(Math.round(viewportWidth() * defaultRatio), minWidth, panelMaxWidth());
  const loadWidth = () => clamp(load<number>(storageKey, defaultWidth()), minWidth, panelMaxWidth());

  const [width, setWidth] = useState(loadWidth);
  const [expanded, setExpanded] = useState(false);
  const widthRef = useRef(width);
  widthRef.current = width;

  const effectiveWidth = expanded ? panelMaxWidth() : width;

  const toggleExpanded = useCallback(() => {
    setExpanded((value) => !value);
  }, []);

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
    expanded,
    toggleExpanded,
    handleResizeStart,
  };
}

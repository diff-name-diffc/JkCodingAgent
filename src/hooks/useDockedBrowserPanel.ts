import { useCallback, useRef, useState } from "react";
import type React from "react";
import { load, save } from "../utils";

const BROWSER_PANEL_MIN_WIDTH = 420;
const BROWSER_PANEL_DEFAULT_RATIO = 0.4;
const BROWSER_PANEL_MAX_RATIO = 0.75;

function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value));
}

function viewportWidth(): number {
  return typeof window === "undefined" ? 1280 : window.innerWidth;
}

function browserPanelMaxWidth(): number {
  return Math.max(BROWSER_PANEL_MIN_WIDTH, Math.floor(viewportWidth() * BROWSER_PANEL_MAX_RATIO));
}

function defaultBrowserPanelWidth(): number {
  return clamp(
    Math.round(viewportWidth() * BROWSER_PANEL_DEFAULT_RATIO),
    BROWSER_PANEL_MIN_WIDTH,
    browserPanelMaxWidth(),
  );
}

function loadBrowserPanelWidth(storageKey: string): number {
  const stored = load<number>(storageKey, defaultBrowserPanelWidth());
  return clamp(stored, BROWSER_PANEL_MIN_WIDTH, browserPanelMaxWidth());
}

export function useDockedBrowserPanel(storageKey: string) {
  const [width, setWidth] = useState(() => loadBrowserPanelWidth(storageKey));
  const [expanded, setExpanded] = useState(false);
  const widthRef = useRef(width);
  widthRef.current = width;

  const effectiveWidth = expanded ? browserPanelMaxWidth() : width;

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
        nextWidth = clamp(
          startWidth + (startX - ev.clientX),
          BROWSER_PANEL_MIN_WIDTH,
          browserPanelMaxWidth(),
        );
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
    [storageKey],
  );

  return {
    effectiveWidth,
    expanded,
    toggleExpanded,
    handleResizeStart,
  };
}

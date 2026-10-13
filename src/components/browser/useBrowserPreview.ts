import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import type { BrowserFrameEvent } from "../../types";
import {
  freezeBrowserPreview,
  getBrowserPreviewFrame,
  observeBrowserPreview,
} from "./browser-preview-channel";
import { PreviewCanvas } from "./preview-canvas";
import { usePreviewVisibility } from "./usePreviewVisibility";

interface BrowserPreviewOptions {
  sessionId: string | null;
  toolCallId?: string | null;
  enabled: boolean;
  finished?: boolean;
  containerRef: RefObject<HTMLElement | null>;
  /** 通用浏览器面板只画 Canvas，不因每一帧而重新渲染整个面板。 */
  trackFrame?: boolean;
}

export function useBrowserPreview({
  sessionId,
  toolCallId = null,
  enabled,
  finished = false,
  containerRef,
  trackFrame = true,
}: BrowserPreviewOptions) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const latest = useRef<BrowserFrameEvent | null>(null);
  const [frame, setFrame] = useState<BrowserFrameEvent | null>(null);
  const [error, setError] = useState<string | null>(null);
  const painter = useRef<PreviewCanvas | null>(null);
  const visible = usePreviewVisibility(containerRef);
  const state = useRef({ enabled, finished, visible, trackFrame });
  state.current = { enabled, finished, visible, trackFrame };

  // 身份切换在绘制前清屏，旧 Image.onload 即使迟到也不能写进新会话。
  useLayoutEffect(() => {
    const surface = new PreviewCanvas(setError);
    painter.current = surface;
    surface.clear(canvasRef.current);
    latest.current = sessionId ? getBrowserPreviewFrame({ sessionId, toolCallId }) : null;
    setFrame(latest.current);
    setError(null);
    return () => {
      surface.clear();
      painter.current = null;
    };
  }, [sessionId, toolCallId]);

  useLayoutEffect(() => {
    if (sessionId && (visible || finished)) {
      const scope = { sessionId, toolCallId };
      const cached = getBrowserPreviewFrame(scope);
      if (cached && latest.current !== cached) {
        latest.current = cached;
        if (trackFrame) setFrame(cached);
      }
      if (finished && toolCallId) freezeBrowserPreview(scope);
    }
  }, [finished, sessionId, toolCallId, trackFrame, visible]);

  // Canvas 可能随面板状态重新挂载；相同帧由 painter 去重，不重复解码。
  useLayoutEffect(() => {
    if (!visible) {
      painter.current?.pause();
      return;
    }
    const canvas = canvasRef.current;
    if (canvas && latest.current) painter.current?.present(canvas, latest.current);
  });

  useEffect(() => {
    if (!sessionId || !visible || !enabled || finished) return;
    const scope = { sessionId, toolCallId };
    let active = true;
    // frame 状态只服务 URL/时间戳等文案信息，按 200ms 节流推送，
    // 高频帧流不得逐帧触发组件重渲染（Canvas 绘制走 painter 即时路径）。
    let lastFramePushAt = 0;
    const unsubscribe = observeBrowserPreview({
      scope,
      onFrame: (next) => {
        const current = state.current;
        if (
          !active ||
          !current.enabled ||
          current.finished ||
          !current.visible ||
          document.visibilityState !== "visible"
        )
          return;
        latest.current = next;
        const canvas = canvasRef.current;
        if (canvas) painter.current?.present(canvas, next);
        const now = performance.now();
        if (current.trackFrame && now - lastFramePushAt >= 200) {
          lastFramePushAt = now;
          setFrame(next);
        }
        setError(null);
      },
      onError: (message) => {
        // null 是租约续租成功的恢复信号，也要透传以清除过期错误。
        if (active) setError(message);
      },
    });
    return () => {
      active = false;
      unsubscribe();
    };
  }, [enabled, finished, sessionId, toolCallId, visible]);

  return { canvasRef, frame, error, visible };
}

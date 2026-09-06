import { useCallback, useEffect, useRef, useState } from "react";
import type { RefObject } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { BrowserFrameEvent, BrowserLogEvent, BrowserStatus } from "../../types";

/** 日志保留上限（与拆分前一致：slice(-30) + 新行）。 */
const MAX_LOG_LINES = 31;

export interface BrowserPanelSession {
  status: BrowserStatus | null;
  logs: string[];
  error: string | null;
  canvasRef: RefObject<HTMLCanvasElement | null>;
  setStatus: (status: BrowserStatus | null) => void;
  setError: (error: string | null) => void;
  appendLog: (message: string) => void;
  refreshStatus: () => Promise<void>;
}

/**
 * 浏览器面板会话态（UI-18 自 BrowserPanel 拆出）：状态/日志/错误 +
 * screencast 帧绘制 + 三个 Tauri 事件监听（均按 sessionId 过滤）。
 *
 * 串帧修复：sessionId 变化时立即清空画布与日志/错误/状态——旧会话的
 * 最后一帧不再残留到新会话首帧到达之前（验收条款「切会话不串帧」）。
 */
export function useBrowserPanelSession(
  sessionId: string | null,
  active: boolean,
): BrowserPanelSession {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [status, setStatus] = useState<BrowserStatus | null>(null);
  const [logs, setLogs] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const imageRef = useRef<HTMLImageElement>(new Image());

  const appendLog = useCallback((message: string) => {
    setLogs((prev) => [...prev.slice(-(MAX_LOG_LINES - 1)), message]);
  }, []);

  const drawFrame = useCallback((frame: BrowserFrameEvent) => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const image = imageRef.current;
    image.onload = () => {
      canvas.width = frame.width || image.width;
      canvas.height = frame.height || image.height;
      ctx.drawImage(image, 0, 0, canvas.width, canvas.height);
    };
    image.src = frame.data;
  }, []);

  const refreshStatus = useCallback(async () => {
    if (!sessionId) return;
    try {
      const next = await invoke<BrowserStatus>("browser_get_status", { sessionId });
      setStatus(next);
    } catch (reason) {
      setError(String(reason));
    }
  }, [sessionId]);

  // 会话切换即清屏（含首次挂载时的无害重置），随后拉取新会话状态。
  useEffect(() => {
    const canvas = canvasRef.current;
    if (canvas) {
      canvas.width = 0;
      canvas.height = 0;
    }
    setLogs([]);
    setError(null);
    setStatus(null);
    if (sessionId) void refreshStatus().catch(console.error);
  }, [sessionId, refreshStatus]);

  // 面板重新可见时刷新状态（保活切回兜底）。
  useEffect(() => {
    if (!active || !sessionId) return;
    refreshStatus().catch(console.error);
  }, [active, refreshStatus, sessionId]);

  useEffect(() => {
    if (!sessionId) return;
    const unsubs = [
      listen<BrowserStatus>("browser-status", (event) => {
        if (event.payload.sessionId === sessionId) {
          setStatus(event.payload);
          if (event.payload.message) appendLog(event.payload.message);
        }
      }),
      listen<BrowserFrameEvent>("browser-frame", (event) => {
        if (event.payload.sessionId === sessionId) {
          drawFrame(event.payload);
        }
      }),
      listen<BrowserLogEvent>("browser-log", (event) => {
        if (event.payload.sessionId === sessionId) {
          appendLog(event.payload.message);
        }
      }),
    ];

    return () => {
      unsubs.forEach((unsub) => unsub.then((fn) => fn()).catch(() => {}));
    };
  }, [appendLog, drawFrame, sessionId]);

  return { status, logs, error, canvasRef, setStatus, setError, appendLog, refreshStatus };
}

import { useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AlertCircle, LoaderCircle, Monitor, Square } from "lucide-react";
import type { SubAgentSession } from "../subAgentEventStore";
import { Button } from "../ui/button";
import { useBrowserPreview } from "./useBrowserPreview";

interface BrowserAgentMonitorProps {
  sessionId: string;
  toolCallId: string;
  status: SubAgentSession["status"];
}

const TERMINAL_LABELS = {
  completed: "执行已完成",
  failed: "执行已失败",
  cancelled: "执行已取消",
  timed_out: "执行已超时",
} as const;

/** 仅观察当前委派执行；画面与轨迹文本分流，避免逐帧更新聊天工作台。 */
export function BrowserAgentMonitor({ sessionId, toolCallId, status }: BrowserAgentMonitorProps) {
  const containerRef = useRef<HTMLElement>(null);
  const cancelInFlight = useRef(false);
  const [stopping, setStopping] = useState(false);
  const [stopError, setStopError] = useState<string | null>(null);
  const running = status === "running";
  const { canvasRef, frame, error, visible } = useBrowserPreview({
    sessionId,
    toolCallId,
    enabled: running,
    finished: !running,
    containerRef,
  });
  const stateLabel = !running
    ? `${TERMINAL_LABELS[status]}${frame ? " · 最后画面" : ""}`
    : stopping
      ? "等待停止确认"
      : !visible
        ? "监控已暂停"
        : frame
          ? "实时监控"
          : "等待画面";
  const capturedAt = frame ? new Date(frame.capturedAt) : null;

  async function cancelExploration() {
    if (!running || cancelInFlight.current) return;
    cancelInFlight.current = true;
    setStopping(true);
    setStopError(null);
    try {
      await invoke<void>("browser_cancel_exploration", { sessionId, toolCallId });
      // 请求完成不代表执行已停止，终态必须来自子智能体轨迹。
    } catch (reason) {
      cancelInFlight.current = false;
      setStopping(false);
      setStopError(reason instanceof Error ? reason.message : String(reason));
    }
  }

  return (
    <section
      ref={containerRef}
      aria-label="浏览器只读监控"
      className="min-w-0 w-full max-w-full overflow-hidden rounded-lg border border-border bg-background"
    >
      <div className="flex min-w-0 items-start justify-between gap-2 border-b border-border bg-muted/30 px-3 py-2">
        <div className="flex min-w-0 flex-1 items-start gap-2 py-1">
          <Monitor aria-hidden className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          <div className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-0.5">
            <span className="shrink-0 whitespace-nowrap text-xs font-medium text-foreground">
              只读监控
            </span>
            <span className="text-[11px] text-muted-foreground" role="status">
              {stateLabel}
            </span>
          </div>
        </div>
        {running && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            disabled={stopping}
            aria-label="停止本次浏览器探索"
            title="停止本次浏览器探索"
            onClick={() => void cancelExploration()}
            className="h-6 shrink-0 gap-1.5 px-1.5 text-[11px] text-muted-foreground hover:text-destructive [&_svg]:size-3"
          >
            {stopping ? <LoaderCircle className="spin" /> : <Square />}
            {stopping ? "停止中" : "停止"}
          </Button>
        )}
      </div>
      <div className="flex min-w-0 items-center gap-2 border-b border-border/60 px-3 py-1.5">
        <span
          title={frame?.url ?? undefined}
          className="min-w-0 flex-1 truncate font-mono text-[10px] text-muted-foreground"
        >
          {frame?.url || "等待页面地址"}
        </span>
        {capturedAt && (
          <time
            dateTime={capturedAt.toISOString()}
            title={`最新帧：${capturedAt.toLocaleString()}`}
            className="shrink-0 font-mono text-[10px] tabular-nums text-muted-foreground"
          >
            {capturedAt.toLocaleTimeString([], { hour12: false })}
          </time>
        )}
      </div>
      <div
        className="relative min-h-32 w-full min-w-0 max-h-80 overflow-hidden bg-muted/20"
        style={{ aspectRatio: frame ? `${frame.width} / ${frame.height}` : "16 / 10" }}
      >
        <canvas
          ref={canvasRef}
          aria-label="本次浏览器探索画面，仅供观察"
          className="absolute inset-0 block h-full w-full object-contain object-center"
        />
        {!frame && (
          <p className="absolute inset-0 m-0 flex items-center justify-center px-4 text-center text-xs text-muted-foreground">
            {running ? "等待本次执行的浏览器画面…" : "本次执行暂无可用画面"}
          </p>
        )}
      </div>
      {(error || stopError) && (
        <div
          role="alert"
          className="min-w-0 space-y-1 border-t border-border px-3 py-2 text-xs text-destructive [overflow-wrap:anywhere]"
        >
          {error && (
            <p className="m-0 flex min-w-0 items-start gap-1.5">
              <AlertCircle aria-hidden className="mt-0.5 h-3 w-3 shrink-0" />
              <span className="min-w-0">画面监控失败：{error}</span>
            </p>
          )}
          {stopError && <p className="m-0">停止请求失败：{stopError}</p>}
        </div>
      )}
    </section>
  );
}

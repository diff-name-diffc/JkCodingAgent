import { useEffect, useRef } from "react";
import { useBrowserPanelSession } from "./useBrowserPanelSession";

interface BrowserTraceViewProps {
  /** 浏览器会话 id（= 工作区 id）；工具卡片从 workspaceId 取。 */
  sessionId: string;
}

/**
 * 执行轨迹内嵌的浏览器实时视图（无头化改造）：
 * Agent 执行浏览器的细节不再弹出面板/窗口，点开工具卡（执行轨迹）时
 * 经 screencast 帧流回放当前画面 + 最近状态。复用 useBrowserPanelSession
 * 的会话态（状态/帧绘制/日志监听均按 sessionId 过滤）。
 */
export function BrowserTraceView({ sessionId }: BrowserTraceViewProps) {
  const session = useBrowserPanelSession(sessionId, true);
  const { status, canvasRef } = session;
  const state = status?.state ?? "closed";
  const connected = state !== "closed" && state !== "page_closed";

  return (
    <section className="rounded-md border border-border/70 bg-background/60 p-2">
      <div className="mb-1.5 flex items-center justify-between gap-2">
        <span className="text-[10px] font-semibold uppercase tracking-[0.12em] text-muted-foreground">
          浏览器实时画面
        </span>
        <span className="min-w-0 truncate text-[11px] text-muted-foreground">
          {status?.url && status.url !== "about:blank" ? status.url : state}
        </span>
      </div>
      <div className="ai-browser-stage" style={{ maxHeight: 320 }}>
        {connected ? (
          <canvas
            ref={canvasRef}
            className="ai-browser-canvas"
            style={{ aspectRatio: "16 / 10", objectFit: "contain" }}
          />
        ) : (
          <div className="flex h-32 items-center justify-center text-[11px] text-muted-foreground">
            浏览器会话未运行（{state}）
          </div>
        )}
      </div>
    </section>
  );
}

interface BrowserActivityFeedProps {
  lines: string[];
  /** 展开态显示更长回看；折叠态只露最新几条（当下执行信息）。 */
  expanded: boolean;
  /** 运行中持续滚到最新；结束后冻结。 */
  active: boolean;
}

const COLLAPSED_LINES = 3;

/** 浏览器执行信息滚动条：消息区里「正在打开 xxx / 正在点击 xxx」的实时时间线。 */
export function BrowserActivityFeed({ lines, expanded, active }: BrowserActivityFeedProps) {
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!active) return;
    const container = containerRef.current;
    if (container) {
      container.scrollTop = container.scrollHeight;
    }
  }, [lines, active]);

  if (lines.length === 0) return null;
  const visible = expanded ? lines : lines.slice(-COLLAPSED_LINES);

  return (
    <div
      ref={containerRef}
      className="chat-scroll max-h-24 overflow-y-auto rounded-md border border-border/60 bg-muted/40 px-2.5 py-1.5 font-mono text-[11px] leading-relaxed text-muted-foreground"
    >
      {visible.map((line, index) => (
        <div key={`${index}-${line.slice(0, 24)}`} className="truncate" title={line}>
          {line}
        </div>
      ))}
    </div>
  );
}

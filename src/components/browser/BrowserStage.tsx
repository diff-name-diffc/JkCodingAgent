import type { MouseEvent, RefObject } from "react";
import { MonitorUp } from "lucide-react";

export interface BrowserStageProps {
  canvasRef: RefObject<HTMLCanvasElement | null>;
  busy: boolean;
  hasSession: boolean;
  connected: boolean;
  pageClosed: boolean;
  minimized: boolean;
  onCanvasClick: (event: MouseEvent<HTMLCanvasElement>) => void;
  onReopen: () => void;
}

/**
 * 浏览器舞台（UI-18 拆分）：screencast 画布 + 最小化浮条 + 空态。
 * 画布 CSS 拉伸铺满，点击坐标映射基于实时 rect（命令层），缩放安全。
 */
export function BrowserStage({
  canvasRef,
  busy,
  hasSession,
  connected,
  pageClosed,
  minimized,
  onCanvasClick,
  onReopen,
}: BrowserStageProps) {
  return (
    <div className="ai-browser-stage">
      {hasSession && connected ? (
        <>
          <canvas
            ref={canvasRef}
            onClick={onCanvasClick}
            className={busy ? "ai-browser-canvas is-busy" : "ai-browser-canvas"}
          />
          {minimized && (
            <div className="ai-browser-minimized">窗口已最小化 · 可在面板中点击操作</div>
          )}
        </>
      ) : (
        <div className="ai-browser-empty">
          {hasSession ? (
            <>
              {pageClosed && (
                <div className="ai-browser-empty-block">
                  <div className="ai-browser-empty-title">浏览器窗口已关闭</div>
                  <button
                    type="button"
                    title="重新打开窗口"
                    onClick={onReopen}
                    disabled={busy}
                    className="ai-browser-reopen-button"
                  >
                    <MonitorUp size={14} />
                    重新打开
                  </button>
                </div>
              )}
              {!connected && !pageClosed && (
                <div className="ai-browser-empty-copy">点击上方 ⚡ 按钮启动浏览器</div>
              )}
            </>
          ) : (
            <div className="ai-browser-empty-copy">选择一个会话后可启动浏览器</div>
          )}
        </div>
      )}
    </div>
  );
}

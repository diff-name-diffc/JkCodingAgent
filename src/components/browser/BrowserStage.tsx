import type { MouseEvent, RefObject } from "react";

export interface BrowserStageProps {
  canvasRef: RefObject<HTMLCanvasElement | null>;
  busy: boolean;
  hasSession: boolean;
  connected: boolean;
  pageClosed: boolean;
  onCanvasClick: (event: MouseEvent<HTMLCanvasElement>) => void;
}

/**
 * 浏览器舞台（UI-18 拆分）：screencast 画布 + 空态。
 * 画布 CSS 拉伸铺满，点击坐标映射基于实时 rect（命令层），缩放安全。
 */
export function BrowserStage({
  canvasRef,
  busy,
  hasSession,
  connected,
  pageClosed,
  onCanvasClick,
}: BrowserStageProps) {
  return (
    <div className="ai-browser-stage">
      {hasSession && connected ? (
        <canvas
          ref={canvasRef}
          onClick={onCanvasClick}
          className={busy ? "ai-browser-canvas is-busy" : "ai-browser-canvas"}
        />
      ) : (
        <div className="ai-browser-empty">
          {hasSession ? (
            <>
              {pageClosed && (
                <div className="ai-browser-empty-title">浏览器页面已关闭</div>
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

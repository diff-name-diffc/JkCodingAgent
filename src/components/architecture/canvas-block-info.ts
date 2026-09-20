/**
 * 画布阻断状态：渲染崩溃的识别与回传给 Agent 的「画布未就绪」报告纯函数。
 *
 * Excalidraw 无许可门禁（tldraw 时代的 LicenseGate / VITE_TLDRAW_LICENSE_KEY
 * 已随迁移整体移除），画布只剩「渲染崩溃」一种阻断，由 ErrorBoundary 捕获。
 */

/** 画布阻断原因：渲染崩溃。 */
export type CanvasBlockInfo = { kind: "crash"; message: string; stack?: string };

/** 纯函数：画布未就绪时回传给 Agent 的报告文案（附阻断原因诊断）。 */
export function canvasNotReadyReport(block: CanvasBlockInfo | null): string {
  if (block?.kind === "crash") {
    return `错误：画布此前发生渲染崩溃（${block.message}），程序未执行。`;
  }
  return "错误：画布未就绪（架构设计视图未打开），程序未执行。";
}

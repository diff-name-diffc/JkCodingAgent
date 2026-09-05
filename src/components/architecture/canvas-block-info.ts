/**
 * tldraw 画布阻断状态：生产许可门禁 / 渲染崩溃 / 意外关闭 的识别常量，
 * 以及回传给 Agent 的「画布未就绪」报告纯函数。
 *
 * 背景：tldraw v5 的 LicenseProvider 在生产构建（NODE_ENV=production 且运行
 * 地址非 localhost http，如打包后的 tauri://localhost）未检测到有效生产许可
 * 证时，许可状态流转为 `unlicensed-production`，挂载约 5 秒后把整个 editor
 * 子树替换为隐藏占位节点（LicenseGate，`data-testid="tl-license-expired"`），
 * 表现为「画布突然关闭」；开发环境（tauri dev → http://localhost）被判定为
 * 开发模式，不触发该门禁。
 */

/** tldraw 生产许可门禁注入的隐藏占位节点（LicenseProvider 的 LicenseGate）。 */
export const TLDR_LICENSE_GATE_SELECTOR = '[data-testid="tl-license-expired"]';

/** 画布阻断原因：许可门禁 / 渲染崩溃 / 意外关闭（未捕获到门禁的兜底）。 */
export type CanvasBlockInfo =
  | { kind: "license" }
  | { kind: "crash"; message: string; stack?: string }
  | { kind: "unexpected" };

/** 纯函数：画布未就绪时回传给 Agent 的报告文案（附阻断原因诊断）。 */
export function canvasNotReadyReport(block: CanvasBlockInfo | null): string {
  if (block?.kind === "license") {
    return "错误：画布已被 tldraw 生产许可校验关闭（未配置有效 licenseKey），程序未执行。请在构建时设置环境变量 VITE_TLDRAW_LICENSE_KEY 后重新打包。";
  }
  if (block?.kind === "crash") {
    return `错误：画布此前发生渲染崩溃（${block.message}），程序未执行。`;
  }
  if (block?.kind === "unexpected") {
    return "错误：画布此前被意外关闭，程序未执行。请重新打开架构设计视图后重试。";
  }
  return "错误：画布未就绪（架构设计视图未打开），程序未执行。";
}

/**
 * 架构助手「本次发送将附带什么」提示文案（UI-15c，纯函数）。
 *
 * 感知开关只表达意图；空画布时截图/快照实际都会被跳过（useArchitectureChat
 * 发送管线按图形数守卫）。本函数把「意图 × 画布状态」合成为用户可读的一行
 * 上下文，让发送前的附带信息可见（设计 §5.6）。
 */

export interface AttachmentContextInput {
  attachScreenshot: boolean;
  attachSnapshot: boolean;
  /** 当前页图形数量（0 = 空画布）。 */
  shapeCount: number;
}

/** 两开关全关时无附带信息，返回 null（调用方不渲染提示行）。 */
export function formatAttachmentHint(input: AttachmentContextInput): string | null {
  const { attachScreenshot, attachSnapshot, shapeCount } = input;
  if (!attachScreenshot && !attachSnapshot) return null;

  if (shapeCount <= 0) {
    const skipped: string[] = [];
    if (attachScreenshot) skipped.push("截图");
    if (attachSnapshot) skipped.push("快照");
    return `画布为空：本次发送不附带${skipped.join("与")}`;
  }

  const parts: string[] = [];
  if (attachScreenshot) parts.push(`截图（${shapeCount} 个图形）`);
  if (attachSnapshot) parts.push("结构化快照");
  return `本次发送将附带：${parts.join(" · ")}`;
}

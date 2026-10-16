/** 架构设计视图专属类型（事件载荷等）。 */

/**
 * `architecture-run-request` 事件载荷（镜像后端
 * `ArchRunRequestPayload`，camelCase）。program 的权威校验在 Rust 侧
 * 已完成，前端执行器再做防御性校验。
 */
export interface ArchRunRequestPayload {
  runId: string;
  workspaceId: string;
  program: unknown;
}

/** 架构会话的内部分类（与后端 `INTERNAL_CHAT_CATEGORY` 同值，勿改）。 */
export const ARCH_DESIGN_CATEGORY = "arch-design";

/** 架构画板文件元数据（镜像后端 `ArchitectureCanvasMeta`，camelCase）。 */
export interface ArchitectureCanvasMeta {
  id: string;
  name: string;
  /** epoch 毫秒。 */
  createdAt: number;
  updatedAt: number;
}

/** 画板文件完整载荷：元数据 + `.excalidraw` 场景 JSON 原文。 */
export interface ArchitectureCanvasFile {
  meta: ArchitectureCanvasMeta;
  dataJson: string;
}

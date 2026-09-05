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

import type { DispatcherToolArtifactRef, DispatcherToolResultMode } from "../../types";

export type ToolCallStatus = "running" | "success" | "error";

export interface ToolCallItem {
  id: string;
  name: string;
  status: ToolCallStatus;
  durationMs?: number;
  input?: unknown;
  output?: unknown;
  errorText?: string;
}

/**
 * 工具调用活动在聊天消息流中的展示模型。
 * 由 dispatcherChatView 的 live/finalized 管道生成，经 tool-call-card 渲染。
 */
export interface ToolActivityItem extends ToolCallItem {
  detailRefs?: DispatcherToolArtifactRef[];
  resultMode?: DispatcherToolResultMode;
  /** 仅用于计算流式工具调用耗时，不属于 ToolCallCard 的展示契约。 */
  startedAtMs?: number;
}

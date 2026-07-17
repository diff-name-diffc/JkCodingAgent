import type { DispatcherToolArtifactRef, DispatcherToolResultMode } from "../../types";

/**
 * 工具调用活动在聊天消息流中的展示模型。
 * 由 dispatcherChatView 的 live/finalized 管道生成，经 tool-call-card 渲染。
 */
export interface ToolActivityItem {
  key: string;
  name: string;
  input?: string;
  displayText?: string;
  detailRefs?: DispatcherToolArtifactRef[];
  resultMode?: DispatcherToolResultMode;
  status: "planned" | "running" | "completed";
  summaryText?: string;
}

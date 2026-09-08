import * as React from "react";
import type {
  DispatcherMessage,
  DispatcherToolArtifactRef,
  ModelCategory,
  PythonCodeRunRecord,
} from "../../types";
import type { AssistantThinkingBlock, AssistantTurnSegment } from "../dispatcher-chat/assistant-segments";
import { buildDispatcherDisplayItems } from "../dispatcherChatView";
import type { ToolActivityItem } from "../dispatcher-chat/tool-activity";
import { AssistantMessage } from "./assistant-message";
import { UserMessage } from "./user-message";

/**
 * One row in the message list. Dispatches to <UserMessage /> or
 * <AssistantMessage /> based on the display-item kind. Memoized so that
 * streaming appends to the trailing live bubble don't re-render every
 * historical row.
 */
export type MessageDisplayItem =
  | { kind: "user"; id: string; message: DispatcherMessage }
  | {
      kind: "assistant";
      id: string;
      segments: AssistantTurnSegment[];
      tools: ToolActivityItem[];
      thinking: AssistantThinkingBlock | null;
      /** 连续 AI 消息分组中仅第一条为 true。 */
      showAvatar: boolean;
      usageStats?: import("../../types").DispatcherMessageUsageStats;
      messageId?: string;
      sourceUserMessage?: DispatcherMessage;
    };

export interface MessageItemProps {
  item: MessageDisplayItem;
  pythonRunRecords?: Record<string, PythonCodeRunRecord>;
  onRunPython?: (target: {
    messageId: string;
    codeBlockIndex: number;
    code: string;
    codeHash: string;
  }) => void;
  onCopyMessage?: (text: string) => void;
  onRegenerateFromMessage?: (message: DispatcherMessage) => void;
  onEditMessage?: (message: DispatcherMessage) => void;
  onOpenArtifact?: (artifact: DispatcherToolArtifactRef) => void;
  onOpenSubAgent?: (tool: ToolActivityItem) => void;
  /** UI-25 第四批遗留：工具级「模型未配置」错误深链（透传至 AssistantMessage）。
   *  调用方须保持身份稳定（本组件为 React.memo）。 */
  onConfigureModel?: (category?: ModelCategory) => void;
  className?: string;
}

export const MessageItem = React.memo(function MessageItem({
  item,
  pythonRunRecords,
  onRunPython,
  onCopyMessage,
  onRegenerateFromMessage,
  onEditMessage,
  onOpenArtifact,
  onOpenSubAgent,
  onConfigureModel,
  className,
}: MessageItemProps) {
  if (item.kind === "user") {
    return (
      <UserMessage
        message={item.message}
        onEdit={onEditMessage}
        className={className}
      />
    );
  }
  const sourceUserMessage = item.sourceUserMessage;
  return (
    <AssistantMessage
      segments={item.segments}
      tools={item.tools}
      thinking={item.thinking}
      usageStats={item.usageStats}
      messageId={item.messageId}
      rowId={item.id}
      showAvatar={item.showAvatar}
      pythonRunRecords={pythonRunRecords}
      onRunPython={onRunPython}
      onCopy={onCopyMessage}
      onRegenerate={
        sourceUserMessage && onRegenerateFromMessage
          ? () => onRegenerateFromMessage(sourceUserMessage)
          : undefined
      }
      onOpenArtifact={onOpenArtifact}
      onOpenSubAgent={onOpenSubAgent}
      onConfigureModel={onConfigureModel}
      className={className}
    />
  );
});

/** Build display items from raw DispatcherMessage[] using the shared view-model layer. */
export function buildItems(messages: DispatcherMessage[]): MessageDisplayItem[] {
  // 复用统一的历史投影构建器：分段归组、工具卡片 upsert 与
  // superseded 正文降级逻辑全部集中在 buildDispatcherDisplayItems。
  const raw = buildDispatcherDisplayItems(messages);
  let prevKind: "user" | "assistant" | null = null;
  let sourceUserMessage: DispatcherMessage | undefined;
  return raw.map((item) => {
    if (item.kind === "user") {
      prevKind = "user";
      sourceUserMessage = item.message;
      return { kind: "user", id: item.id, message: item.message };
    }
    // 连续 AI 消息为一组，仅组内第一条显示头像锚点。
    const showAvatar = prevKind !== "assistant";
    prevKind = "assistant";
    return {
      kind: "assistant",
      id: item.id,
      segments: item.turn.segments,
      tools: item.turn.tools,
      thinking: item.turn.thinking,
      usageStats: item.turn.usageStats,
      showAvatar,
      sourceUserMessage,
    };
  });
}

import * as React from "react";
import type {
  DispatcherMessage,
  DispatcherToolArtifactRef,
  ModelCategory,
  PythonCodeRunRecord,
} from "../../types";
import type { ToolActivityItem } from "../dispatcher-chat/tool-activity";
import { AssistantMessage } from "./assistant-message";
import { UserMessage } from "./user-message";
import type { MessageDisplayItem } from "./message-projection";

/**
 * One row in the message list. Dispatches to <UserMessage /> or
 * <AssistantMessage /> based on the display-item kind. Memoized so that
 * streaming appends to the trailing live bubble don't re-render every
 * historical row.
 */
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
  const sourceUserMessage = item.kind === "assistant" ? item.sourceUserMessage : undefined;
  // 闭包必须在组件内 memo 化：父级 renderRow 在流式期间每帧重建 JSX，
  // 若在此处内联箭头函数，onRegenerate 引用逐帧变化会击穿本组件的
  // React.memo，导致整个历史列表随 token 流逐帧重渲染。
  const handleRegenerate = React.useMemo(
    () =>
      sourceUserMessage && onRegenerateFromMessage
        ? () => onRegenerateFromMessage(sourceUserMessage)
        : undefined,
    [sourceUserMessage, onRegenerateFromMessage],
  );

  if (item.kind === "user") {
    return <UserMessage message={item.message} onEdit={onEditMessage} className={className} />;
  }
  return (
    <AssistantMessage
      segments={item.segments}
      tools={item.tools}
      thinking={item.thinking}
      usageStats={item.usageStats}
      messageId={item.messageId}
      rowId={item.id}
      showAvatar={item.showAvatar}
      isStreaming={item.isStreaming}
      isThinking={item.isThinking}
      placeholder={item.placeholder}
      pythonRunRecords={pythonRunRecords}
      onRunPython={onRunPython}
      onCopy={onCopyMessage}
      onRegenerate={handleRegenerate}
      onOpenArtifact={onOpenArtifact}
      onOpenSubAgent={onOpenSubAgent}
      onConfigureModel={onConfigureModel}
      className={className}
    />
  );
});

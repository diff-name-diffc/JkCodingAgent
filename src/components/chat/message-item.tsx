import * as React from "react";
import type {
  DispatcherMessage,
  DispatcherToolArtifactRef,
  PythonCodeRunRecord,
} from "../../types";
import type {
  AssistantThinkingBlock,
  AssistantTurnSegment,
} from "../dispatcherChatView";
import { buildDispatcherDisplayItems } from "../dispatcherChatView";
import type { ToolActivityItem } from "../ToolActivityBubble";
import { cn } from "../../lib/cn";
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
      usageStats?: import("../../types").DispatcherMessageUsageStats;
      messageId?: string;
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
  onRegenerate?: () => void;
  onOpenArtifact?: (artifact: DispatcherToolArtifactRef) => void;
  onOpenSubAgent?: (tool: ToolActivityItem) => void;
  className?: string;
}

export const MessageItem = React.memo(function MessageItem({
  item,
  pythonRunRecords,
  onRunPython,
  onCopyMessage,
  onRegenerate,
  onOpenArtifact,
  onOpenSubAgent,
  className,
}: MessageItemProps) {
  if (item.kind === "user") {
    return <UserMessage message={item.message} className={className} />;
  }
  return (
    <AssistantMessage
      segments={item.segments}
      tools={item.tools}
      thinking={item.thinking}
      usageStats={item.usageStats}
      messageId={item.messageId}
      pythonRunRecords={pythonRunRecords}
      onRunPython={onRunPython}
      onCopy={onCopyMessage}
      onRegenerate={onRegenerate}
      onOpenArtifact={onOpenArtifact}
      onOpenSubAgent={onOpenSubAgent}
      className={className}
    />
  );
});

/** Build display items from raw DispatcherMessage[] using the existing view-model layer. */
export function buildItems(messages: DispatcherMessage[]): MessageDisplayItem[] {
  // Defer to the existing, well-tested builder so segment-grouping, tool
  // upserting, and superseded-text logic stay identical to the legacy surface.
  const raw = buildDispatcherDisplayItems(messages);
  return raw.map((item) =>
    item.kind === "user"
      ? { kind: "user", id: item.id, message: item.message }
      : {
          kind: "assistant",
          id: item.id,
          segments: item.turn.segments,
          tools: item.turn.tools,
          thinking: item.turn.thinking,
          usageStats: item.turn.usageStats,
        },
  );
}

export const messageItemClass = cn("px-4 py-3");

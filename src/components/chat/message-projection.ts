import type { DispatcherMessage, DispatcherMessageUsageStats } from "../../types";
import type { DispatcherLiveSessionState } from "../dispatcherSessionStore";
import { buildDispatcherDisplayItems } from "../dispatcherChatView";
import type {
  AssistantThinkingBlock,
  AssistantTurnSegment,
} from "../dispatcher-chat/assistant-segments";
import { upsertToolActivity, type ToolActivityItem } from "../dispatcher-chat/tool-activity";

export interface AssistantMessageDisplayItem {
  kind: "assistant";
  id: string;
  segments: AssistantTurnSegment[];
  tools: ToolActivityItem[];
  thinking: AssistantThinkingBlock | null;
  showAvatar: boolean;
  usageStats?: DispatcherMessageUsageStats;
  messageId?: string;
  sourceUserMessage?: DispatcherMessage;
  persistedMessageIds: string[];
  isStreaming?: boolean;
  isThinking?: boolean;
  placeholder?: string | null;
}

export type MessageDisplayItem =
  { kind: "user"; id: string; message: DispatcherMessage } | AssistantMessageDisplayItem;

const EMPTY_TOOLS: ToolActivityItem[] = [];
// 正文 token 不改变工具数组；仅在任一工具快照变化时重建卡片，保持 memo 有效。
const toolProjectionCache = new WeakMap<
  ToolActivityItem[],
  WeakMap<ToolActivityItem[], ToolActivityItem[]>
>();

/** 历史与实时共用用户轮次身份；完成快照到达时保留原来的行与交互状态。 */
export function buildItems(messages: DispatcherMessage[]): MessageDisplayItem[] {
  let previousKind: MessageDisplayItem["kind"] | null = null;
  let sourceUserMessage: DispatcherMessage | undefined;
  return buildDispatcherDisplayItems(messages).map((item) => {
    if (item.kind === "user") {
      previousKind = "user";
      sourceUserMessage = item.message;
      return item;
    }
    const showAvatar = previousKind !== "assistant";
    previousKind = "assistant";
    return {
      kind: "assistant",
      id: item.id,
      segments: item.turn.segments,
      tools: item.turn.tools,
      thinking: item.turn.thinking,
      usageStats: item.turn.usageStats,
      // 轮次内缺自身 messageId 的分段（合成说明段）以最后一条落库消息为锚，
      // 供 MarkdownRenderer 的 python 运行记录定位回退。
      messageId: item.turn.messageIds[item.turn.messageIds.length - 1],
      persistedMessageIds: item.turn.messageIds,
      showAvatar,
      sourceUserMessage,
    };
  });
}

/** 只替换当前轮次，历史行保持对象身份，避免每个 token 重渲染整份记录。 */
export function projectLiveMessageItems(
  history: MessageDisplayItem[],
  live: DispatcherLiveSessionState | null,
  sessionId: string | null,
): MessageDisplayItem[] {
  if (!live || !sessionId) return history;
  const isStreaming = live.hasPendingRun || live.isLoading;
  if (
    !isStreaming &&
    live.streamingSegments.length === 0 &&
    live.liveToolCalls.length === 0 &&
    !live.liveThinking &&
    !live.assistantPlaceholder
  ) {
    return history;
  }

  const last = history[history.length - 1];
  const sourceUserMessage = last?.kind === "user" ? last.message : last?.sourceUserMessage;
  // 会话切换期间历史 props 与订阅快照可能分帧到达，不能把新 live 接到旧会话。
  if (sourceUserMessage && sourceUserMessage.workspaceId !== sessionId) return history;
  const persisted = last?.kind === "assistant" ? last : undefined;
  const item: AssistantMessageDisplayItem = {
    kind: "assistant",
    id: persisted?.id ?? `assistant-turn-${sourceUserMessage?.id ?? sessionId}`,
    showAvatar: persisted?.showAvatar ?? true,
    sourceUserMessage,
    persistedMessageIds: persisted?.persistedMessageIds ?? [],
    usageStats: persisted?.usageStats,
    messageId: persisted?.messageId,
    segments: mergeTurnSegments(persisted?.segments ?? [], live.streamingSegments),
    tools: mergeTurnTools(persisted?.tools ?? EMPTY_TOOLS, live.liveToolCalls),
    thinking: mergeTurnThinking(persisted, live.liveThinking),
    isStreaming,
    isThinking: Boolean(
      isStreaming &&
      live.liveThinking &&
      !persisted?.persistedMessageIds.includes(live.liveThinking.messageId ?? "") &&
      !live.streamingSegments.some(
        (segment) =>
          segment.kind === "assistant-text" &&
          segment.messageId === live.liveThinking?.messageId &&
          segment.text.trim(),
      ),
    ),
    placeholder: live.assistantPlaceholder,
  };
  return persisted ? [...history.slice(0, -1), item] : [...history, item];
}

function segmentKey(segment: AssistantTurnSegment): string | undefined {
  if (segment.kind === "assistant-text") {
    return segment.messageId ? `message:${segment.messageId}` : undefined;
  }
  return segment.toolCallId ? `tool:${segment.toolCallId}` : undefined;
}

function mergeTurnSegments(
  persisted: AssistantTurnSegment[],
  live: AssistantTurnSegment[],
): AssistantTurnSegment[] {
  const segments = [...persisted];
  const positions = new Map<string, number>();
  segments.forEach((segment, index) => {
    const key = segmentKey(segment);
    if (key) positions.set(key, index);
  });
  for (const segment of live) {
    const key = segmentKey(segment);
    const index = key ? positions.get(key) : undefined;
    if (index === undefined) {
      if (key) positions.set(key, segments.length);
      segments.push(segment);
      continue;
    }
    const saved = segments[index];
    segments[index] = {
      ...segment,
      ...saved,
      // 历史查询可能只取得流式前缀；只接受同一消息的后续增量，不重复拼接。
      text: segment.text.startsWith(saved.text) ? segment.text : saved.text,
      superseded: saved.superseded || segment.superseded,
    };
  }
  return segments;
}

function mergeTurnTools(persisted: ToolActivityItem[], live: ToolActivityItem[]) {
  if (live.length === 0) return persisted;
  if (persisted.length === 0) return live;
  const cached = toolProjectionCache.get(persisted)?.get(live);
  if (cached) return cached;
  const tools = [...persisted];
  for (const tool of live) {
    const saved = tools.find((item) => item.id === tool.id);
    upsertToolActivity(tools, tool);
    // 已落库的终态不能被较早的 planned/started 快照重新变成「执行中」。
    if (saved && saved.status !== "running" && tool.status === "running") {
      upsertToolActivity(tools, { ...saved, planned: false });
    }
  }
  const liveSnapshots =
    toolProjectionCache.get(persisted) ?? new WeakMap<ToolActivityItem[], ToolActivityItem[]>();
  liveSnapshots.set(live, tools);
  toolProjectionCache.set(persisted, liveSnapshots);
  return tools;
}

function mergeTurnThinking(
  persisted: AssistantMessageDisplayItem | undefined,
  live: AssistantThinkingBlock | null,
): AssistantThinkingBlock | null {
  const saved = persisted?.thinking;
  if (!live || (live.messageId && persisted?.persistedMessageIds.includes(live.messageId))) {
    return saved ?? null;
  }
  if (!saved) return live;
  return {
    text: `${saved.text}\n\n${live.text}`,
    elapsedMs: saved.elapsedMs + live.elapsedMs,
  };
}

/**
 * 助手轮次的内容分段模型：正文/工具摘要两类段的流式追加、阶段判定与展示分组。
 *
 * 分段是渲染与流式合并共用的中间形态；历史投影（`../dispatcherChatView`）
 * 与实时事件处理（`useDispatcherActions` / `useLiveSessionState`）都经过这里。
 */

import type { DispatcherToolResultMode } from "../../types";

export interface AssistantTurnSegment {
  kind: "assistant-text" | "tool-summary";
  text: string;
  messageId?: string;
  // assistant-text 专用：明确发起工具调用的阶段说明，或模型切换等系统进度。
  superseded?: boolean;
  toolCallId?: string;
  toolName?: string;
  resultMode?: DispatcherToolResultMode;
}

export interface AssistantThinkingBlock {
  text: string;
  elapsedMs: number;
  /** 实时思考所属消息，用于与已持久化的同一消息去重。 */
  messageId?: string;
}

export function appendAssistantTextSegment(
  segments: AssistantTurnSegment[],
  delta: string,
  messageId?: string,
): AssistantTurnSegment[] {
  return appendSegmentText(segments, {
    kind: "assistant-text",
    text: delta,
    messageId,
  });
}

/** 工具规划已确认时，只标记发起该批工具的消息，不降级其他渐进回复。 */
export function markAssistantTextAsProgress(
  segments: AssistantTurnSegment[],
  messageId: string,
): AssistantTurnSegment[] {
  return segments.map((segment) =>
    segment.kind === "assistant-text" && segment.messageId === messageId && !segment.superseded
      ? { ...segment, superseded: true }
      : segment,
  );
}

/** 只分组助手文字；工具摘要沿用独立展示与去重规则，保留分段身份和顺序。 */
export function partitionAssistantSegments(segments: AssistantTurnSegment[]): {
  progressSegments: AssistantTurnSegment[];
  answerSegments: AssistantTurnSegment[];
} {
  const progressSegments: AssistantTurnSegment[] = [];
  const answerSegments: AssistantTurnSegment[] = [];
  for (const segment of segments) {
    if (segment.kind !== "assistant-text" || !segment.text.trim()) continue;
    (segment.superseded ? progressSegments : answerSegments).push(segment);
  }
  return { progressSegments, answerSegments };
}

export function appendToolSummarySegment(
  segments: AssistantTurnSegment[],
  payload: {
    toolCallId: string;
    name: string;
    delta: string;
    resultMode: DispatcherToolResultMode;
  },
): AssistantTurnSegment[] {
  return appendSegmentText(segments, {
    kind: "tool-summary",
    text: payload.delta,
    toolCallId: payload.toolCallId,
    toolName: payload.name,
    resultMode: payload.resultMode,
  });
}

export function appendSegmentText(
  segments: AssistantTurnSegment[],
  incoming: AssistantTurnSegment,
): AssistantTurnSegment[] {
  const nextSegments = [...segments];
  const matches = (segment: AssistantTurnSegment) =>
    segment.kind === incoming.kind &&
    segment.messageId === incoming.messageId &&
    Boolean(segment.superseded) === Boolean(incoming.superseded) &&
    (incoming.kind !== "tool-summary" ||
      (segment.toolCallId ?? segment.toolName) === (incoming.toolCallId ?? incoming.toolName));
  // 并行工具摘要可能穿插在正文增量之间，已知身份的分段要继续写回原段。
  // 没有身份的旧载荷只允许相邻合并，避免把独立消息误合并。
  const hasIdentity = incoming.messageId || incoming.toolCallId;
  const index = hasIdentity
    ? nextSegments.findIndex(matches)
    : nextSegments.length > 0 && matches(nextSegments[nextSegments.length - 1])
      ? nextSegments.length - 1
      : -1;

  if (index >= 0) {
    const current = nextSegments[index];
    nextSegments[index] = {
      ...current,
      text: `${current.text}${incoming.text}`,
      resultMode: incoming.resultMode ?? current.resultMode,
      toolCallId: incoming.toolCallId ?? current.toolCallId,
      toolName: incoming.toolName ?? current.toolName,
    };
    return nextSegments;
  }

  nextSegments.push(incoming);
  return nextSegments;
}

/** 就地追加（历史投影在可变 turn 上装配分段时使用）。 */
export function pushAssistantSegment(
  segments: AssistantTurnSegment[],
  incoming: AssistantTurnSegment,
) {
  const next = appendSegmentText(segments, incoming);
  segments.splice(0, segments.length, ...next);
}

/** 可内联为正文分段的工具结果模式（其余模式只进工具卡片）。 */
export function shouldRenderToolSummaryInline(
  mode: DispatcherToolResultMode | undefined,
): mode is Exclude<DispatcherToolResultMode, "raw" | "pending_summary" | "truncated"> {
  return (
    mode === "summary" ||
    mode === "conservative_summary" ||
    mode === "intent_compressed" ||
    mode === "structured_fallback"
  );
}

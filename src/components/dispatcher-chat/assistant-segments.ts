/**
 * 助手轮次的内容分段模型：正文/工具摘要两类段的流式追加、降级与展示净化。
 *
 * 分段是渲染与流式合并共用的中间形态；历史投影（`../dispatcherChatView`）
 * 与实时事件处理（`useDispatcherActions` / `useLiveSessionState`）都经过这里。
 */

import type { DispatcherToolResultMode } from "../../types";

export interface AssistantTurnSegment {
  kind: "assistant-text" | "tool-summary";
  text: string;
  messageId?: string;
  // assistant-text 专用：标记为已被后续正文覆盖的前置正文。
  // 渲染为置灰折叠条而非最终正文气泡。流式中由 demoteActiveTextSegments
  // 在下一轮 assistantStarted 时设置；历史重载时由带 tool_calls 的消息
  // 在 buildDispatcherDisplayItems 中设置。
  superseded?: boolean;
  toolCallId?: string;
  toolName?: string;
  resultMode?: DispatcherToolResultMode;
}

export interface AssistantThinkingBlock {
  text: string;
  elapsedMs: number;
}

export function appendAssistantTextSegment(
  segments: AssistantTurnSegment[],
  delta: string,
): AssistantTurnSegment[] {
  return appendSegmentText(segments, {
    kind: "assistant-text",
    text: delta,
  });
}

/**
 * Mark every active (non-superseded) assistant-text segment as superseded.
 *
 * Used at the start of a new assistant round (assistantStarted / assistantMessage)
 * to demote the previously-streamed reply into a collapsed grey block, instead
 * of discarding it. tool-summary segments are left untouched so ongoing tool
 * summaries keep accumulating.
 */
export function demoteActiveTextSegments(segments: AssistantTurnSegment[]): AssistantTurnSegment[] {
  return segments.map((segment) =>
    segment.kind === "assistant-text" && !segment.superseded
      ? { ...segment, superseded: true }
      : segment,
  );
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
  const lastSegment = nextSegments[nextSegments.length - 1];
  const matchesLastSegment =
    lastSegment &&
    lastSegment.kind === incoming.kind &&
    // A superseded segment must never merge with an active one (or vice versa):
    // keep them as separate blocks so the prior reply stays collapsible while
    // the new live reply accumulates on its own segment.
    Boolean(lastSegment.superseded) === Boolean(incoming.superseded) &&
    (incoming.kind !== "tool-summary" ||
      (lastSegment.toolCallId ?? lastSegment.toolName) ===
        (incoming.toolCallId ?? incoming.toolName));

  if (matchesLastSegment) {
    nextSegments[nextSegments.length - 1] = {
      ...lastSegment,
      text: `${lastSegment.text}${incoming.text}`,
      resultMode: incoming.resultMode ?? lastSegment.resultMode,
      toolCallId: incoming.toolCallId ?? lastSegment.toolCallId,
      toolName: incoming.toolName ?? lastSegment.toolName,
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

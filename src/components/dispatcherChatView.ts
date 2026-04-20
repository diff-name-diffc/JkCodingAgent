import type {
  DispatcherMessage,
  DispatcherToolArtifactRef,
  DispatcherToolResultMode,
} from "../types";
import type { ToolActivityItem } from "./ToolActivityBubble";

interface OutboundToolCall {
  id?: string;
  function?: {
    name?: string;
    arguments?: string;
  };
}

interface DispatcherAssistantTurn {
  id: string;
  tools: ToolActivityItem[];
  segments: AssistantTurnSegment[];
}

export interface AssistantTurnSegment {
  kind: "assistant-text" | "tool-summary";
  text: string;
  toolCallId?: string;
  toolName?: string;
  resultMode?: DispatcherToolResultMode;
}

type DispatcherDisplayItem =
  | { kind: "user"; id: string; message: DispatcherMessage }
  | { kind: "assistant"; id: string; turn: DispatcherAssistantTurn };

export function buildDispatcherDisplayItems(
  messages: DispatcherMessage[],
): DispatcherDisplayItem[] {
  const items: DispatcherDisplayItem[] = [];
  let currentTurn: DispatcherAssistantTurn | null = null;

  const ensureAssistantTurn = (seedId: string) => {
    if (currentTurn) {
      return currentTurn;
    }

    currentTurn = {
      id: `assistant-turn-${seedId}`,
      tools: [],
      segments: [],
    };
    items.push({
      kind: "assistant",
      id: currentTurn.id,
      turn: currentTurn,
    });
    return currentTurn;
  };

  for (const message of messages) {
    if (message.role === "user") {
      currentTurn = null;
      items.push({
        kind: "user",
        id: message.id,
        message,
      });
      continue;
    }

    const turn = ensureAssistantTurn(message.id);

    if (message.role === "assistant") {
      const toolCalls = parseToolCalls(message.toolCallsJson);
      for (const toolCall of toolCalls) {
        upsertToolActivity(turn.tools, {
          key: toolCall.id || `${message.id}-${toolCall.name}`,
          name: toolCall.name || "tool",
          input: prettyPrintToolPayload(toolCall.arguments),
          status: "completed",
        });
      }

      const content = message.content.trim();
      if (content) {
        pushAssistantSegment(turn.segments, {
          kind: "assistant-text",
          text: content,
        });
      }
      continue;
    }

    if (message.role === "tool") {
      const shouldInlineSummary = shouldRenderToolSummaryInline(message.toolResultMode);
      const displayText = shouldInlineSummary ? undefined : message.content;

      upsertToolActivity(turn.tools, {
        key: message.toolCallId || `${message.id}-${message.toolName || "tool"}`,
        name: message.toolName || "tool",
        displayText,
        detailRefs: message.toolArtifacts,
        resultMode: message.toolResultMode,
        status: "completed",
      });

      const content = message.content.trim();
      if (shouldInlineSummary && content) {
        pushAssistantSegment(turn.segments, {
          kind: "tool-summary",
          text: content,
          toolCallId: message.toolCallId,
          toolName: message.toolName,
          resultMode: message.toolResultMode,
        });
      }
    }
  }

  return items.filter(
    (item) =>
      item.kind === "user" ||
      item.turn.tools.length > 0 ||
      item.turn.segments.some((segment) => segment.text.trim()),
  );
}

export function startLiveToolActivity(
  tools: ToolActivityItem[],
  payload: { toolCallId?: string; name: string; arguments: string },
): ToolActivityItem[] {
  const nextTools = [...tools];
  const key = payload.toolCallId || createLiveToolKey(payload.name, nextTools.length);
  upsertToolActivity(nextTools, {
    key,
    name: payload.name,
    input: prettyPrintToolPayload(payload.arguments),
    status: "running",
  });
  return nextTools;
}

export function finishLiveToolActivity(
  tools: ToolActivityItem[],
  payload: {
    toolCallId?: string;
    name: string;
    displayText: string;
    resultMode: DispatcherToolResultMode;
    detailRefs: DispatcherToolArtifactRef[];
  },
): ToolActivityItem[] {
  const nextTools = [...tools];
  const byToolCallId = payload.toolCallId
    ? nextTools.findIndex((tool) => tool.key === payload.toolCallId)
    : -1;
  let byRunningName = -1;
  for (let index = nextTools.length - 1; index >= 0; index -= 1) {
    const tool = nextTools[index];
    if (tool.name === payload.name && tool.status === "running") {
      byRunningName = index;
      break;
    }
  }
  const matchIndex = byToolCallId >= 0 ? byToolCallId : byRunningName;

  if (matchIndex >= 0) {
    nextTools[matchIndex] = {
      ...nextTools[matchIndex],
      displayText: shouldRenderToolSummaryInline(payload.resultMode)
        ? undefined
        : payload.displayText,
      detailRefs: payload.detailRefs,
      resultMode: payload.resultMode,
      status: "completed",
    };
    return nextTools;
  }

  nextTools.push({
    key: payload.toolCallId || createLiveToolKey(payload.name, nextTools.length),
    name: payload.name,
    displayText: shouldRenderToolSummaryInline(payload.resultMode) ? undefined : payload.displayText,
    detailRefs: payload.detailRefs,
    resultMode: payload.resultMode,
    status: "completed",
  });
  return nextTools;
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

export function appendToolSummarySegment(
  segments: AssistantTurnSegment[],
  payload: {
    toolCallId?: string;
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

function appendSegmentText(
  segments: AssistantTurnSegment[],
  incoming: AssistantTurnSegment,
): AssistantTurnSegment[] {
  const nextSegments = [...segments];
  const lastSegment = nextSegments[nextSegments.length - 1];
  const matchesLastSegment =
    lastSegment &&
    lastSegment.kind === incoming.kind &&
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

function parseToolCalls(raw: string | undefined): Array<{
  id?: string;
  name?: string;
  arguments?: string;
}> {
  if (!raw) {
    return [];
  }

  try {
    const parsed = JSON.parse(raw) as OutboundToolCall[];
    return parsed.map((item) => ({
      id: item.id,
      name: item.function?.name,
      arguments: item.function?.arguments,
    }));
  } catch {
    return [];
  }
}

function upsertToolActivity(tools: ToolActivityItem[], incoming: ToolActivityItem) {
  const index = tools.findIndex((tool) => tool.key === incoming.key);
  if (index < 0) {
    tools.push(incoming);
    return;
  }

  tools[index] = {
    ...tools[index],
    ...incoming,
    input: incoming.input ?? tools[index].input,
    displayText: incoming.displayText ?? tools[index].displayText,
    detailRefs: incoming.detailRefs ?? tools[index].detailRefs,
  };
}

function prettyPrintToolPayload(raw: string | undefined): string {
  if (!raw) {
    return "";
  }

  try {
    const parsed = JSON.parse(raw);
    return JSON.stringify(parsed, null, 2);
  } catch {
    return raw;
  }
}

function createLiveToolKey(name: string, index: number): string {
  return `live-${name}-${index}-${Date.now()}`;
}

function pushAssistantSegment(segments: AssistantTurnSegment[], incoming: AssistantTurnSegment) {
  const next = appendSegmentText(segments, incoming);
  segments.splice(0, segments.length, ...next);
}

function shouldRenderToolSummaryInline(
  mode: DispatcherToolResultMode | undefined,
): mode is Exclude<DispatcherToolResultMode, "raw"> {
  return mode === "summary" || mode === "conservative_summary";
}

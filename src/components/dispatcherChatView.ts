import type {
  BrowserStatus,
  DispatcherMessage,
  DispatcherMessageUsageStats,
  DispatcherToolArtifactRef,
  DispatcherToolResultMode,
  DispatcherToolRunRecord,
} from "../types";
import {
  mergeToolRunRecords,
  toolRunStatusToCallStatus,
  type ToolActivityItem,
} from "./dispatcher-chat/tool-activity";

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
  thinking: AssistantThinkingBlock | null;
  usageStats?: DispatcherMessageUsageStats;
}

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

type DispatcherDisplayItem =
  | { kind: "user"; id: string; message: DispatcherMessage }
  | { kind: "assistant"; id: string; turn: DispatcherAssistantTurn };

export function buildDispatcherDisplayItems(
  messages: DispatcherMessage[],
): DispatcherDisplayItem[] {
  const items: DispatcherDisplayItem[] = [];
  const toolStartedAt = new Map<string, number>();
  let currentTurn: DispatcherAssistantTurn | null = null;

  const ensureAssistantTurn = (seedId: string) => {
    if (currentTurn) {
      return currentTurn;
    }

    currentTurn = {
      id: `assistant-turn-${seedId}`,
      tools: [],
      segments: [],
      thinking: null,
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
      mergeTurnUsageStats(turn, message.usageStats);
      mergeTurnThinking(turn, message.thinkingContent, message.thinkingElapsedMs);

      const toolCalls = parseToolCalls(message.toolCallsJson);
      for (const toolCall of toolCalls) {
        const id = toolCall.id || `${message.id}-${toolCall.name}`;
        toolStartedAt.set(id, Date.parse(message.createdAt));
        upsertToolActivity(turn.tools, {
          id,
          name: toolCall.name || "tool",
          workspaceId: message.workspaceId,
          input: prettyPrintToolPayload(toolCall.arguments),
          status: "running",
        });
      }

      // When the assistant message contains tool calls, its text content is
      // preliminary reasoning that will be superseded by the follow-up
      // response after tool execution.  Instead of discarding it, mark it as
      // superseded so it renders as a collapsed grey block inside the turn,
      // preserving the intermediate reasoning for the user to expand.
      const hasToolCalls = toolCalls.length > 0;
      const content = message.content.trim();
      if (content) {
        pushAssistantSegment(turn.segments, {
          kind: "assistant-text",
          text: content,
          messageId: message.id,
          superseded: hasToolCalls || undefined,
        });
      }
      continue;
    }

    if (message.role === "tool") {
      const shouldInlineSummary = shouldRenderToolSummaryInline(message.toolResultMode);
      const output = message.content;
      const id =
        message.toolCallId ||
        [...turn.tools]
          .reverse()
          .find((tool) => tool.name === (message.toolName || "tool") && tool.status === "running")
          ?.id ||
        `${message.id}-${message.toolName || "tool"}`;
      const errorText = getToolErrorText(message.content);
      const startedAtMs = toolStartedAt.get(id);
      const finishedAtMs = Date.parse(message.createdAt);

      upsertToolActivity(turn.tools, {
        id,
        name: message.toolName || "tool",
        workspaceId: message.workspaceId,
        output,
        errorText,
        durationMs:
          startedAtMs != null && Number.isFinite(finishedAtMs)
            ? Math.max(0, finishedAtMs - startedAtMs)
            : undefined,
        detailRefs: message.toolArtifacts,
        resultMode: message.toolResultMode,
        status: errorText ? "error" : "success",
      });

      const content = message.content.trim();
      if (shouldInlineSummary && content) {
        pushAssistantSegment(turn.segments, {
          kind: "tool-summary",
          text: content,
          messageId: message.id,
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
      Boolean(item.turn.thinking?.text.trim()) ||
      item.turn.tools.length > 0 ||
      item.turn.segments.some((segment) => segment.text.trim()),
  );
}

export function startLiveToolActivity(
  tools: ToolActivityItem[],
  payload: { toolCallId: string; name: string; arguments: string; workspaceId?: string },
): ToolActivityItem[] {
  const nextTools = [...tools];
  // G9-07：后端保证 toolCallId 必填（Planned→Started→Finished 贯穿同一 id），
  // 不再需要按名称回溯匹配计划中的条目。
  upsertToolActivity(nextTools, {
    id: payload.toolCallId,
    name: payload.name,
    workspaceId: payload.workspaceId,
    input: prettyPrintToolPayload(payload.arguments),
    status: "running",
    startedAtMs: Date.now(),
  });
  return nextTools;
}

export function planLiveToolActivity(
  tools: ToolActivityItem[],
  payload: { toolCallId: string; name: string; arguments: string; workspaceId?: string },
): ToolActivityItem[] {
  const nextTools = [...tools];
  upsertToolActivity(nextTools, {
    id: payload.toolCallId,
    name: payload.name,
    workspaceId: payload.workspaceId,
    input: prettyPrintToolPayload(payload.arguments),
    status: "running",
    startedAtMs: Date.now(),
  });
  return nextTools;
}

export function finishLiveToolActivity(
  tools: ToolActivityItem[],
  payload: {
    toolCallId: string;
    name: string;
    arguments: string;
    displayText: string;
    resultMode: DispatcherToolResultMode;
    detailRefs: DispatcherToolArtifactRef[];
    workspaceId?: string;
  },
): ToolActivityItem[] {
  const nextTools = [...tools];
  const matchIndex = nextTools.findIndex((tool) => tool.id === payload.toolCallId);

  if (matchIndex >= 0) {
    const current = nextTools[matchIndex];
    const errorText = getToolErrorText(payload.displayText);
    nextTools[matchIndex] = {
      ...current,
      workspaceId: current.workspaceId ?? payload.workspaceId,
      output: payload.displayText,
      errorText,
      durationMs: current.startedAtMs == null ? undefined : Date.now() - current.startedAtMs,
      detailRefs: payload.detailRefs,
      resultMode: payload.resultMode,
      status: errorText ? "error" : "success",
    };
    return nextTools;
  }

  // 兜底：对应的 Planned/Started 事件未被处理（如 run 切换）时直接落一条完成态。
  const errorText = getToolErrorText(payload.displayText);
  nextTools.push({
    id: payload.toolCallId,
    name: payload.name,
    workspaceId: payload.workspaceId,
    output: payload.displayText,
    errorText,
    detailRefs: payload.detailRefs,
    resultMode: payload.resultMode,
    status: errorText ? "error" : "success",
  });
  return nextTools;
}

/**
 * 将运行台账事件归并到对应的外层模型工具调用。
 * 子运行只进入该卡片的 toolRuns，不会生成新的聊天消息或顶层工具卡片。
 */
export function updateLiveToolRunActivity(
  tools: ToolActivityItem[],
  run: DispatcherToolRunRecord,
): ToolActivityItem[] {
  const isRootRun = !run.parentRunId;
  const matchIndex = isRootRun
    ? tools.findIndex(
        (tool) =>
          tool.id === run.toolCallId ||
          tool.runId === run.id ||
          tool.toolRuns?.some((current) => current.id === run.id),
      )
    : tools.findIndex(
        (tool) =>
          tool.runId === run.parentRunId ||
          tool.toolRuns?.some((current) => current.id === run.parentRunId),
      );

  if (matchIndex < 0) {
    if (!isRootRun) return tools;
    const status = toolRunStatusToCallStatus(run.status);
    return [
      ...tools,
      {
        id: run.toolCallId,
        name: run.toolName,
        workspaceId: run.workspaceId,
        runId: run.id,
        toolRuns: [run],
        input: prettyPrintToolPayload(run.effectiveArgumentsJson || run.argumentsJson),
        errorText: run.errorMessage ?? undefined,
        durationMs: status === "running" ? undefined : run.durationMs,
        status,
      },
    ];
  }

  const nextTools = [...tools];
  const current = nextTools[matchIndex];
  const toolRuns = mergeToolRunRecords(current.toolRuns ?? [], [run]);
  if (!isRootRun) {
    nextTools[matchIndex] = {
      ...current,
      workspaceId: current.workspaceId ?? run.workspaceId,
      toolRuns,
    };
    return nextTools;
  }

  const status = toolRunStatusToCallStatus(run.status);
  const startedAtMs = run.startedAt ? Date.parse(run.startedAt) : Number.NaN;
  nextTools[matchIndex] = {
    ...current,
    name: run.toolName,
    workspaceId: run.workspaceId,
    runId: run.id,
    toolRuns,
    input: current.input ?? prettyPrintToolPayload(run.effectiveArgumentsJson || run.argumentsJson),
    status,
    durationMs: status === "running" ? current.durationMs : run.durationMs,
    errorText: run.errorMessage ?? current.errorText,
    resultMode: run.resultMode ?? current.resultMode,
    startedAtMs: Number.isFinite(startedAtMs) ? startedAtMs : current.startedAtMs,
  };
  return nextTools;
}

export function updateLiveBrowserToolActivity(
  tools: ToolActivityItem[],
  status: BrowserStatus,
): ToolActivityItem[] {
  const message = status.message?.trim() || browserStateLabel(status.state);
  if (!message) return tools;

  const nextTools = [...tools];
  for (let index = nextTools.length - 1; index >= 0; index -= 1) {
    const tool = nextTools[index];
    if (tool.status === "running" && tool.name.startsWith("browser_")) {
      nextTools[index] = {
        ...tool,
        output: message,
      };
      return nextTools;
    }
  }
  return tools;
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

function appendSegmentText(
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
  const index = tools.findIndex((tool) => tool.id === incoming.id);
  if (index < 0) {
    tools.push(incoming);
    return;
  }

  tools[index] = {
    ...tools[index],
    ...incoming,
    input: incoming.input ?? tools[index].input,
    output: incoming.output ?? tools[index].output,
    errorText: incoming.errorText ?? tools[index].errorText,
    durationMs: incoming.durationMs ?? tools[index].durationMs,
    detailRefs: incoming.detailRefs ?? tools[index].detailRefs,
    resultMode: incoming.resultMode ?? tools[index].resultMode,
    workspaceId: incoming.workspaceId ?? tools[index].workspaceId,
    runId: incoming.runId ?? tools[index].runId,
    toolRuns:
      incoming.toolRuns == null
        ? tools[index].toolRuns
        : mergeToolRunRecords(tools[index].toolRuns ?? [], incoming.toolRuns),
    startedAtMs: tools[index].startedAtMs ?? incoming.startedAtMs,
  };
}

function getToolErrorText(output: string): string | undefined {
  const trimmed = output.trim();
  if (/^(错误：|错误:|error:|failed:|失败：|失败:)/i.test(trimmed)) return trimmed;

  try {
    const parsed = JSON.parse(trimmed) as unknown;
    if (parsed && typeof parsed === "object" && "error" in parsed) {
      const error = (parsed as { error?: unknown }).error;
      if (typeof error === "string" && error.trim()) return error.trim();
    }
  } catch {
    // 普通文本结果不是异常，只有明确错误前缀或 error 字段才进入失败态。
  }

  return undefined;
}

function browserStateLabel(state: string): string {
  switch (state) {
    case "starting":
      return "正在启动浏览器";
    case "launching":
      return "正在启动有头浏览器";
    case "downloading":
      return "正在下载浏览器资源";
    case "busy":
      return "正在执行浏览器操作";
    case "ready":
      return "浏览器已就绪";
    case "closed":
      return "浏览器已关闭";
    default:
      return state;
  }
}

function mergeTurnUsageStats(
  turn: DispatcherAssistantTurn,
  incoming: DispatcherMessageUsageStats | null | undefined,
) {
  if (!incoming) {
    return;
  }

  if (!turn.usageStats) {
    turn.usageStats = { ...incoming };
    return;
  }

  turn.usageStats = {
    // Usage snapshots are cumulative for one user turn. Summing multiple
    // assistant snapshots double-counts earlier model calls.
    promptTokens: Math.max(turn.usageStats.promptTokens, incoming.promptTokens),
    completionTokens: Math.max(turn.usageStats.completionTokens, incoming.completionTokens),
    totalTokens: Math.max(turn.usageStats.totalTokens, incoming.totalTokens),
    elapsedMs: Math.max(turn.usageStats.elapsedMs, incoming.elapsedMs),
  };
}

function mergeTurnThinking(
  turn: DispatcherAssistantTurn,
  content: string | null | undefined,
  elapsedMs: number | null | undefined,
) {
  const text = content?.trim();
  if (!text) {
    return;
  }

  if (!turn.thinking) {
    turn.thinking = {
      text,
      elapsedMs: elapsedMs ?? 0,
    };
    return;
  }

  turn.thinking = {
    text: `${turn.thinking.text}\n\n${text}`,
    elapsedMs: turn.thinking.elapsedMs + (elapsedMs ?? 0),
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

function pushAssistantSegment(segments: AssistantTurnSegment[], incoming: AssistantTurnSegment) {
  const next = appendSegmentText(segments, incoming);
  segments.splice(0, segments.length, ...next);
}

/**
 * 压缩工具结果的展示文本净化。
 *
 * 摘要模型按协议输出 `<DISPLAY_SUMMARY>`（前端展示）与 `<CONTEXT_PAYLOAD>`
 * （模型上下文）双区块；早期后端解析兜底会把带标签的原始输出整体持久化，
 * 导致协议标签与模型专用内容直接渲染进聊天界面。前端只应展示
 * DISPLAY_SUMMARY 区块；无协议标签的普通文本原样返回。
 */
export function toolSummaryDisplayText(text: string): string {
  if (!text.includes("<DISPLAY_SUMMARY>") && !text.includes("<CONTEXT_PAYLOAD>")) {
    return text;
  }

  const display = extractTaggedBlock(text, "DISPLAY_SUMMARY", "CONTEXT_PAYLOAD");
  if (display) return display;

  const context = extractTaggedBlock(text, "CONTEXT_PAYLOAD", "DISPLAY_SUMMARY");
  if (context) return context;

  return stripDualSummaryTags(text);
}

function extractTaggedBlock(text: string, tag: string, otherTag: string): string | null {
  const start = text.indexOf(`<${tag}>`);
  if (start < 0) return null;
  const rest = text.slice(start + tag.length + 2);
  const endCandidates = [`</${tag}>`, `<${otherTag}>`]
    .map((marker) => rest.indexOf(marker))
    .filter((index) => index >= 0);
  const end = endCandidates.length > 0 ? Math.min(...endCandidates) : rest.length;
  const block = rest.slice(0, end).trim();
  return block || null;
}

function stripDualSummaryTags(text: string): string {
  return text
    .replace(/<\/?DISPLAY_SUMMARY>/g, "")
    .replace(/<\/?CONTEXT_PAYLOAD>/g, "")
    .trim();
}

function shouldRenderToolSummaryInline(
  mode: DispatcherToolResultMode | undefined,
): mode is Exclude<DispatcherToolResultMode, "raw" | "pending_summary" | "truncated"> {
  return (
    mode === "summary" ||
    mode === "conservative_summary" ||
    mode === "intent_compressed" ||
    mode === "structured_fallback"
  );
}

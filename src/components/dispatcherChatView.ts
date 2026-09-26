/**
 * Dispatcher 聊天的历史消息投影：把持久化消息重建为「用户消息 / 助手轮次」
 * 展示序列。
 *
 * 实时流式侧的工具卡片归并在 `dispatcher-chat/live-tool-activity`，分段模型
 * 与摘要展示净化在 `dispatcher-chat/assistant-segments`。
 */

import type { DispatcherMessage, DispatcherMessageUsageStats } from "../types";
import {
  getToolErrorText,
  prettyPrintToolPayload,
  upsertToolActivity,
} from "./dispatcher-chat/live-tool-activity";
import {
  pushAssistantSegment,
  shouldRenderToolSummaryInline,
  type AssistantThinkingBlock,
  type AssistantTurnSegment,
} from "./dispatcher-chat/assistant-segments";
import type { ToolActivityItem } from "./dispatcher-chat/tool-activity";

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

type DispatcherDisplayItem =
  | { kind: "user"; id: string; message: DispatcherMessage }
  | { kind: "assistant"; id: string; turn: DispatcherAssistantTurn };

export function buildDispatcherDisplayItems(
  messages: DispatcherMessage[],
): DispatcherDisplayItem[] {
  const items: DispatcherDisplayItem[] = [];
  const toolStartedAt = new Map<string, number>();
  const taskCalls = new Map<string, { id: string; name: string }>();
  // R54：toolTaskId → 发起工具的回合。停止运行后落库的清理/完成消息按 createdAt
  // 可能排到下一条 user 消息之后，凭此映射归并回原始回合，避免造出只含完成态
  // 卡片的幽灵回合（查不到归属时回退当前回合）。
  const taskTurns = new Map<string, DispatcherAssistantTurn>();
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

    if (message.role === "assistant") {
      const turn = ensureAssistantTurn(message.id);
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

    if (message.role === "runtime") {
      const taskId = message.toolTaskId;
      const task = taskId ? taskCalls.get(taskId) : undefined;
      if (!taskId || !task) continue;
      let payload: unknown;
      try { payload = JSON.parse(message.contextPayload ?? message.content); } catch { continue; }
      if (!payload || typeof payload !== "object" || !("kind" in payload) || payload.kind !== "tool_completion") continue;
      const output = "context_payload" in payload && typeof payload.context_payload === "string" ? payload.context_payload : message.content;
      const succeeded = "status" in payload && payload.status === "succeeded";
      // 归属优先：完成消息可能排在新 user 消息之后，必须写回发起工具的原始回合。
      const turn = taskTurns.get(taskId) ?? ensureAssistantTurn(message.id);
      // 与 tool 角色分支同口径：完成时刻 − 卡片开始时刻，覆盖 accepted 受理时
      // 落上的部分耗时。
      const startedAtMs = toolStartedAt.get(task.id);
      const finishedAtMs = Date.parse(message.createdAt);
      upsertToolActivity(turn.tools, {
        id: task.id, name: task.name, workspaceId: message.workspaceId,
        output, status: succeeded ? "success" : "error",
        errorText: succeeded ? undefined : output,
        durationMs:
          startedAtMs != null && Number.isFinite(finishedAtMs)
            ? Math.max(0, finishedAtMs - startedAtMs)
            : undefined,
        resultMode: message.toolResultMode, detailRefs: message.toolArtifacts,
      });
      continue;
    }

    if (message.role === "tool") {
      const shouldInlineSummary = shouldRenderToolSummaryInline(message.toolResultMode);
      // contextPayload 是后端真正回灌给 Agent 的内容。工具卡片必须展示它，
      // 不能用面向用户的短摘要冒充模型实际输入。
      const output = message.contextPayload ?? message.content;
      // G9-07 保证 toolCallId 必填，与助手侧 planned 卡片（toolCall.id）直接匹配；
      // 生成式兜底与助手侧 `${message.id}-${name}` 形态对称，仅防极端脏载荷。
      const id = message.toolCallId || `${message.id}-${message.toolName || "tool"}`;
      // 与 runtime 分支同规则：已登记归属的 task 写回原始回合；首次见到时登记。
      const turn =
        (message.toolTaskId ? taskTurns.get(message.toolTaskId) : undefined) ??
        ensureAssistantTurn(message.id);
      if (message.toolTaskId) {
        taskCalls.set(message.toolTaskId, { id, name: message.toolName || "tool" });
        taskTurns.set(message.toolTaskId, turn);
      }
      const accepted = message.toolResultMode === "accepted";
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
        status: accepted ? "running" : errorText ? "error" : "success",
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

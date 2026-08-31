/**
 * 实时工具活动归并：把流式事件（Planned/Started/Finished、运行台账、浏览器状态）
 * 折叠为聊天轮次的工具卡片列表。
 *
 * 与历史消息投影（`../dispatcherChatView` 的 buildDispatcherDisplayItems）共用
 * 本模块的 upsert/错误判定/参数美化助手，保证实时卡片与历史卡片同口径。
 */

import type {
  BrowserStatus,
  DispatcherToolArtifactRef,
  DispatcherToolResultMode,
  DispatcherToolRunRecord,
} from "../../types";
import {
  mergeToolRunRecords,
  toolRunStatusToCallStatus,
  type ToolActivityItem,
} from "./tool-activity";

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
    contextPayload: string;
    resultMode: DispatcherToolResultMode;
    detailRefs: DispatcherToolArtifactRef[];
    workspaceId?: string;
  },
): ToolActivityItem[] {
  const nextTools = [...tools];
  const matchIndex = nextTools.findIndex((tool) => tool.id === payload.toolCallId);

  if (matchIndex >= 0) {
    const current = nextTools[matchIndex];
    const errorText = getToolErrorText(payload.contextPayload);
    nextTools[matchIndex] = {
      ...current,
      workspaceId: current.workspaceId ?? payload.workspaceId,
      output: payload.contextPayload,
      errorText,
      durationMs: current.startedAtMs == null ? undefined : Date.now() - current.startedAtMs,
      detailRefs: payload.detailRefs,
      resultMode: payload.resultMode,
      status: errorText ? "error" : "success",
    };
    return nextTools;
  }

  // 兜底：对应的 Planned/Started 事件未被处理（如 run 切换）时直接落一条完成态。
  const errorText = getToolErrorText(payload.contextPayload);
  nextTools.push({
    id: payload.toolCallId,
    name: payload.name,
    workspaceId: payload.workspaceId,
    output: payload.contextPayload,
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

export function upsertToolActivity(tools: ToolActivityItem[], incoming: ToolActivityItem) {
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

export function getToolErrorText(output: string): string | undefined {
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

export function prettyPrintToolPayload(raw: string | undefined): string {
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

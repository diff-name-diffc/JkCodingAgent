/**
 * Dispatcher Agent 事件通道工厂：把后端 Channel 事件流映射为
 * `dispatcherSessionStore` 的 live state 更新。
 *
 * 由 `useDispatcherActions`（主聊天/项目聊天）与架构设计聊天面板共用——
 * 两者的事件语义完全一致，仅会话/运行定位与用量刷新回调不同。
 */

import { invoke, Channel } from "@tauri-apps/api/core";
import type { DispatcherAgentEvent, DispatcherMessageWire } from "../../types";
import {
  appendAssistantTextSegment,
  appendToolSummarySegment,
  demoteActiveTextSegments,
} from "./assistant-segments";
import {
  planLiveToolActivity,
  startLiveToolActivity,
  finishLiveToolActivity,
  updateLiveToolRunActivity,
} from "./live-tool-activity";
import {
  clearDispatcherActiveRunId,
  createIdleLiveSessionState,
  getDispatcherActiveRunId,
  notifyDispatcherMessages,
} from "../dispatcherSessionStore";
import type { LiveSessionUpdater } from "./useLiveSessionState";

export interface DispatcherEventChannelDeps {
  targetSessionId: string;
  runId: number;
  updateLiveSessionState: LiveSessionUpdater;
  refreshSessionTokenUsage: (targetSessionId?: string) => Promise<void>;
}

export function createDispatcherEventChannel({
  targetSessionId,
  runId,
  updateLiveSessionState,
  refreshSessionTokenUsage,
}: DispatcherEventChannelDeps): Channel<DispatcherAgentEvent> {
  const onEvent = new Channel<DispatcherAgentEvent>();
  onEvent.onmessage = (event) => {
    const isActiveRun = getDispatcherActiveRunId(targetSessionId) === runId;
    switch (event.event) {
      case "started":
        break;
      case "assistantStarted":
        if (!isActiveRun) return;
        updateLiveSessionState(targetSessionId, (state) => ({
          ...state,
          assistantPlaceholder: "正在分析问题...",
          liveThinking: null,
          // Demote the previous reply into a collapsed grey block instead
          // of discarding it, so the user can still expand and read it.
          streamingSegments: demoteActiveTextSegments(state.streamingSegments),
        }));
        break;
      case "modelSwitched":
        if (!isActiveRun) return;
        updateLiveSessionState(targetSessionId, (state) => ({
          ...state,
          assistantPlaceholder: `已检测到图片，自动切换到视觉模型 ${event.data.toModel}。`,
          streamingSegments: appendAssistantTextSegment(
            state.streamingSegments,
            `> ${event.data.reason}，已从 ${event.data.fromModel} 自动切换到视觉模型 ${event.data.toModel}。\n\n`,
          ),
        }));
        break;
      case "userMessage":
        if (!isActiveRun || event.data.message.workspaceId !== targetSessionId) return;
        notifyDispatcherMessages(targetSessionId, [event.data.message]);
        break;
      case "assistantDelta":
        if (!isActiveRun) return;
        // G9-08：事件携带同一 messageId 内单调递增的 seq，可用于去重/乱序
        // 校验。Tauri Channel 保证单通道有序投递，此处按到达顺序追加即可，
        // seq 暂不额外消费。
        updateLiveSessionState(targetSessionId, (state) => ({
          ...state,
          assistantPlaceholder: null,
          streamingSegments: appendAssistantTextSegment(state.streamingSegments, event.data.delta),
        }));
        break;
      case "assistantThinkingDelta":
        if (!isActiveRun) return;
        updateLiveSessionState(targetSessionId, (state) => ({
          ...state,
          assistantPlaceholder: null,
          liveThinking: {
            text: `${state.liveThinking?.text ?? ""}${event.data.delta}`,
            elapsedMs: event.data.elapsedMs,
          },
        }));
        break;
      case "assistantMessage":
        if (!isActiveRun || event.data.message.workspaceId !== targetSessionId) return;
        updateLiveSessionState(targetSessionId, (state) => ({
          ...state,
          assistantPlaceholder: null,
          liveThinking: null,
          streamingSegments: demoteActiveTextSegments(state.streamingSegments),
        }));
        notifyDispatcherMessages(targetSessionId, [event.data.message]);
        break;
      case "runUsageUpdated":
        if (!isActiveRun || event.data.workspaceId !== targetSessionId) return;
        {
          const now = Date.now();
          updateLiveSessionState(targetSessionId, (state) => ({
            ...state,
            activeUsageStats: event.data.stats,
            activeUsageStatsReceivedAt: now,
            usageClockNow: now,
          }));
        }
        void refreshSessionTokenUsage(targetSessionId);
        break;
      case "toolPlanned":
        if (!isActiveRun) return;
        updateLiveSessionState(targetSessionId, (state) => ({
          ...state,
          assistantPlaceholder: "正在规划工具调用...",
          liveToolCalls: planLiveToolActivity(state.liveToolCalls, {
            ...event.data,
            workspaceId: targetSessionId,
          }),
        }));
        break;
      case "toolStarted":
        if (!isActiveRun) return;
        updateLiveSessionState(targetSessionId, (state) => ({
          ...state,
          assistantPlaceholder: "正在执行工具...",
          liveToolCalls: startLiveToolActivity(state.liveToolCalls, {
            ...event.data,
            workspaceId: targetSessionId,
          }),
        }));
        break;
      case "toolSummaryStarted":
        // 有意 no-op：摘要内容由后续 toolSummaryDelta 事件流式追加，
        // 本事件仅标记摘要开始，无需状态更新。
        if (!isActiveRun) return;
        break;
      case "toolSummaryDelta":
        if (!isActiveRun) return;
        updateLiveSessionState(targetSessionId, (state) => ({
          ...state,
          streamingSegments: appendToolSummarySegment(state.streamingSegments, event.data),
        }));
        break;
      case "toolFinished":
        if (!isActiveRun) return;
        updateLiveSessionState(targetSessionId, (state) => ({
          ...state,
          liveToolCalls: finishLiveToolActivity(state.liveToolCalls, {
            ...event.data,
            workspaceId: targetSessionId,
          }),
        }));
        break;
      case "toolRunUpdated":
        if (!isActiveRun || event.data.run.workspaceId !== targetSessionId) return;
        updateLiveSessionState(targetSessionId, (state) => ({
          ...state,
          liveToolCalls: updateLiveToolRunActivity(state.liveToolCalls, event.data.run),
        }));
        break;
      case "finished":
        if (!isActiveRun || event.data.workspaceId !== targetSessionId) return;
        // G7-11：Finished 为轻量负载（workspaceId + messageCount）；此处改调
        // dispatcher_list_messages 拉全量，经 mergeDispatcherMessages 按 id
        // 合并刷新（与 dispatcher-session-updated 的重载路径一致）。
        void invoke<DispatcherMessageWire[]>("dispatcher_list_messages", {
          workspaceId: targetSessionId,
        })
          .then((fresh) => {
            // 竞态守卫：list_messages 在途期间若已开启新 run，过期全量快照
            // 不得推给新 run（merge 只增不删，可能把已删消息加回来）。
            if (getDispatcherActiveRunId(targetSessionId) !== undefined) return;
            if (fresh.length !== event.data.messageCount) {
              console.warn(
                `Finished 对账不一致：后端 ${event.data.messageCount} 条，拉取到 ${fresh.length} 条`,
              );
            }
            notifyDispatcherMessages(targetSessionId, fresh);
          })
          .catch((err) => console.error("Finished 后刷新消息失败:", err));
        void refreshSessionTokenUsage(targetSessionId);
        clearDispatcherActiveRunId(targetSessionId);
        updateLiveSessionState(targetSessionId, () => createIdleLiveSessionState());
        break;
      case "failed":
        if (!isActiveRun || event.data.workspaceId !== targetSessionId) return;
        clearDispatcherActiveRunId(targetSessionId);
        updateLiveSessionState(targetSessionId, () => ({
          ...createIdleLiveSessionState(),
          runError: event.data.message,
        }));
        break;
      default: {
        // 穷尽性检查：后端新增事件变体时编译期报错，而非静默忽略。
        const exhaustive: never = event;
        console.warn("未处理的 dispatcher 事件类型:", exhaustive);
        break;
      }
    }
  };
  return onEvent;
}

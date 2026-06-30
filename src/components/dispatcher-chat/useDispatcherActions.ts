import { useRef, useCallback } from "react";
import { invoke, Channel } from "@tauri-apps/api/core";
import type {
  AgentType,
  ChecklistPlanState,
  DispatchFeedbackState,
  DispatcherAgentEvent,
  DispatcherAgentTurn,
  DispatcherMode,
  DispatcherSessionRuntimeState,
  ImageSegment,
  PlanInteraction,
} from "../../types";
import {
  appendAssistantTextSegment,
  appendToolSummarySegment,
  demoteActiveTextSegments,
  planLiveToolActivity,
  startLiveToolActivity,
  finishLiveToolActivity,
} from "../dispatcherChatView";
import {
  clearDispatcherActiveRunId,
  createIdleLiveSessionState,
  getDispatcherActiveRunId,
  nextDispatcherActiveRunId,
  notifyDispatcherMessages,
} from "../dispatcherSessionStore";
import type { LiveSessionUpdater } from "./useLiveSessionState";
import {
  toErrorMessage,
  createEmptyUsageStats,
} from "./dispatcherChatUtils";

export interface DispatcherChatHandle {
  continueWithResult: (
    result: string,
    dispatchState: DispatchFeedbackState,
    targetSessionId?: string,
    dispatchId?: string,
  ) => void;
  applyRuntimeState: (state: DispatcherSessionRuntimeState) => void;
}

export interface UseDispatcherActionsOptions {
  sessionId: string;
  projectPath: string;
  isPlainChat: boolean;
  mode: DispatcherMode;
  updateLiveSessionState: LiveSessionUpdater;
  scrollMessageListToBottom: () => void;
  currentSessionIdRef: React.RefObject<string>;
  refreshSessionTokenUsage: (targetSessionId?: string) => Promise<void>;
  onOpenPlanDocument?: (path: string) => void;
  autoApproveRef: React.RefObject<boolean>;
  onDispatchApprovedRef: React.RefObject<
    | ((
        dispatchId: string,
        agent: AgentType,
        description: string,
        taskPrompt: string,
        permissionMode: string,
        sessionId: string,
      ) => void)
    | undefined
  >;
  onDispatchContinueRef: React.RefObject<
    | ((agent: AgentType, text: string, sessionId: string) => void)
    | undefined
  >;
  onDispatchExitRef: React.RefObject<
    | ((agent: AgentType, reason: string, sessionId: string) => void)
    | undefined
  >;
  // State setters for plan/checklist/mode managed outside this hook
  setMode: (mode: DispatcherMode) => void;
  setChecklist: (checklist: ChecklistPlanState | null) => void;
  setPlanInteraction: (interaction: PlanInteraction | null) => void;
  setActivePlanPath: (path: string | null) => void;
  shouldStickToBottomRef: React.RefObject<boolean>;
  setInput: (value: string) => void;
  setAttachedImages: React.Dispatch<React.SetStateAction<ImageSegment[]>>;
}

export interface UseDispatcherActionsResult {
  enqueueDispatcherRun: (
    targetSessionId: string,
    runner: (onEvent: Channel<DispatcherAgentEvent>) => Promise<void>,
  ) => Promise<void>;
  sendUserMessage: (
    rawText: string,
    images?: ImageSegment[],
    targetSessionId?: string,
    targetMode?: DispatcherMode,
  ) => Promise<void>;
  continueWithResult: (
    result: string,
    dispatchState: DispatchFeedbackState,
    targetSessionId?: string,
    dispatchId?: string,
  ) => Promise<void>;
  applyRuntimeState: (state: DispatcherSessionRuntimeState) => void;
}

export function useDispatcherActions({
  sessionId,
  projectPath,
  isPlainChat,
  mode,
  updateLiveSessionState,
  scrollMessageListToBottom,
  currentSessionIdRef,
  refreshSessionTokenUsage,
  onOpenPlanDocument,
  autoApproveRef,
  onDispatchApprovedRef,
  onDispatchContinueRef,
  onDispatchExitRef,
  setMode,
  setChecklist,
  setPlanInteraction,
  setActivePlanPath,
  shouldStickToBottomRef,
  setInput,
  setAttachedImages,
}: UseDispatcherActionsOptions): UseDispatcherActionsResult {
  const runQueuesRef = useRef<Map<string, Promise<void>>>(new Map());

  const createEventChannel = useCallback(
    (targetSessionId: string, runId: number) => {
      const onEvent = new Channel<DispatcherAgentEvent>();
      onEvent.onmessage = (event) => {
        const isActiveRun = getDispatcherActiveRunId(targetSessionId) === runId;
        const isCurrentSession = currentSessionIdRef.current === targetSessionId;

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
            updateLiveSessionState(targetSessionId, (state) => ({
              ...state,
              assistantPlaceholder: null,
              streamingSegments: appendAssistantTextSegment(
                state.streamingSegments,
                event.data.delta,
              ),
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
              // Demote the previous reply into a collapsed grey block instead
              // of discarding it, so the user can still expand and read it.
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
              liveToolCalls: planLiveToolActivity(state.liveToolCalls, event.data),
            }));
            break;
          case "toolStarted":
            if (!isActiveRun) return;
            updateLiveSessionState(targetSessionId, (state) => ({
              ...state,
              assistantPlaceholder: "正在执行工具...",
              liveToolCalls: startLiveToolActivity(state.liveToolCalls, event.data),
            }));
            break;
          case "toolSummaryStarted":
            if (!isActiveRun) return;
            break;
          case "toolSummaryDelta":
            if (!isActiveRun) return;
            updateLiveSessionState(targetSessionId, (state) => ({
              ...state,
              streamingSegments: appendToolSummarySegment(
                state.streamingSegments,
                event.data,
              ),
            }));
            break;
          case "toolFinished":
            if (!isActiveRun) return;
            updateLiveSessionState(targetSessionId, (state) => ({
              ...state,
              liveToolCalls: finishLiveToolActivity(state.liveToolCalls, event.data),
            }));
            break;
          case "toolRunUpdated":
            if (!isActiveRun || event.data.run.workspaceId !== targetSessionId) return;
            break;
          case "checklistPlanUpdated":
            if (!isActiveRun || !isCurrentSession) return;
            setChecklist(event.data.state);
            break;
          case "planQuestionRequested":
            if (!isActiveRun || !isCurrentSession) return;
            setPlanInteraction(event.data.interaction);
            break;
          case "planDocumentOpened":
            if (!isActiveRun || !isCurrentSession) return;
            setActivePlanPath(event.data.planPath);
            onOpenPlanDocument?.(event.data.planPath);
            break;
          case "planReady":
            if (!isActiveRun || !isCurrentSession) return;
            setPlanInteraction(event.data.interaction);
            if (event.data.interaction.kind === "ready") {
              setActivePlanPath(event.data.interaction.planPath);
              onOpenPlanDocument?.(event.data.interaction.planPath);
            }
            break;
          case "planImplemented":
            if (!isActiveRun || !isCurrentSession) return;
            setActivePlanPath(event.data.implementedPath);
            setPlanInteraction(null);
            onOpenPlanDocument?.(event.data.implementedPath);
            break;
          case "dispatchProposed": {
            const { dispatchId, agent, description, taskPrompt, permissionMode } = event.data;
            if (isPlainChat) return;
            if (autoApproveRef.current && onDispatchApprovedRef.current) {
              onDispatchApprovedRef.current(
                dispatchId,
                agent,
                description,
                taskPrompt,
                permissionMode,
                targetSessionId,
              );
            } else if (isActiveRun) {
              updateLiveSessionState(targetSessionId, (state) => ({
                ...state,
                pendingDispatches: [
                  ...state.pendingDispatches,
                  { dispatchId, agent, description, taskPrompt, permissionMode },
                ],
              }));
            }
            break;
          }
          case "dispatchContinue": {
            onDispatchContinueRef.current?.(event.data.agent, event.data.text, targetSessionId);
            break;
          }
          case "dispatchExit": {
            onDispatchExitRef.current?.(event.data.agent, event.data.reason, targetSessionId);
            break;
          }
          case "finished":
            if (!isActiveRun) return;
            notifyDispatcherMessages(
              targetSessionId,
              event.data.messages.filter((message: { workspaceId: string }) => message.workspaceId === targetSessionId),
            );
            void refreshSessionTokenUsage(targetSessionId);
            clearDispatcherActiveRunId(targetSessionId);
            updateLiveSessionState(targetSessionId, () => createIdleLiveSessionState());
            break;
        }
      };
      return onEvent;
    },
    [
      autoApproveRef,
      currentSessionIdRef,
      isPlainChat,
      onDispatchApprovedRef,
      onDispatchContinueRef,
      onDispatchExitRef,
      onOpenPlanDocument,
      refreshSessionTokenUsage,
      setChecklist,
      setPlanInteraction,
      setActivePlanPath,
      updateLiveSessionState,
    ],
  );

  const enqueueDispatcherRun = useCallback(
    async (
      targetSessionId: string,
      runner: (onEvent: Channel<DispatcherAgentEvent>) => Promise<void>,
    ) => {
      const previous = runQueuesRef.current.get(targetSessionId) ?? Promise.resolve();
      const queued = previous
        .catch(() => undefined)
        .then(async () => {
          const runId = nextDispatcherActiveRunId(targetSessionId);
          const now = Date.now();
          updateLiveSessionState(targetSessionId, () => ({
            ...createIdleLiveSessionState(),
            hasPendingRun: true,
            isLoading: true,
            activeUsageStats: createEmptyUsageStats(),
            activeUsageStatsReceivedAt: now,
            usageClockNow: now,
          }));

          const onEvent = createEventChannel(targetSessionId, runId);

          try {
            await runner(onEvent);
          } finally {
            if (getDispatcherActiveRunId(targetSessionId) === runId) {
              updateLiveSessionState(targetSessionId, (state) => ({
                ...state,
                hasPendingRun: false,
                isLoading: false,
                activeUsageStats: null,
              }));
            }
          }
        });

      runQueuesRef.current.set(targetSessionId, queued);

      try {
        await queued;
      } finally {
        if (runQueuesRef.current.get(targetSessionId) === queued) {
          runQueuesRef.current.delete(targetSessionId);
        }
      }
    },
    [createEventChannel, updateLiveSessionState],
  );

  const sendUserMessage = useCallback(
    async (
      rawText: string,
      images: ImageSegment[] = [],
      targetSessionId = sessionId,
      targetMode: DispatcherMode = mode,
    ) => {
      const text = rawText.trim();
      if (!text && images.length === 0) return;

      setInput("");
      setAttachedImages([]);
      updateLiveSessionState(targetSessionId, (state) => ({
        ...state,
        pendingDispatches: [],
      }));
      if (currentSessionIdRef.current === targetSessionId) {
        shouldStickToBottomRef.current = true;
        window.requestAnimationFrame(() => scrollMessageListToBottom());
      }

      const segments: Array<{ type: string; [key: string]: unknown }> = [];
      for (const img of images) {
        segments.push({ ...img });
      }
      if (text) {
        segments.push({
          id: crypto.randomUUID(),
          type: "text",
          text,
        });
      }
      const segmentsJson = JSON.stringify(segments);

      try {
        await enqueueDispatcherRun(targetSessionId, async (onEvent) => {
          if (isPlainChat) {
            await invoke<DispatcherAgentTurn>("dispatcher_send_plain_chat_message", {
              workspaceId: targetSessionId,
              content: text,
              segmentsJson,
              onEvent,
            });
          } else {
            await invoke<DispatcherAgentTurn>("dispatcher_send_message", {
              workspaceId: targetSessionId,
              projectPath,
              content: text,
              segmentsJson,
              mode: targetMode,
              onEvent,
            });
          }
        });
      } catch (err) {
        console.error("发送消息失败:", err);
        updateLiveSessionState(targetSessionId, (state) => ({
          ...state,
          runError: `${isPlainChat ? "聊天" : "调度智能体"}执行失败：${toErrorMessage(err)}`,
        }));
      }
    },
    [
      currentSessionIdRef,
      enqueueDispatcherRun,
      isPlainChat,
      mode,
      projectPath,
      scrollMessageListToBottom,
      sessionId,
      setAttachedImages,
      setInput,
      shouldStickToBottomRef,
      updateLiveSessionState,
    ],
  );

  const continueWithResult = useCallback(
    async (
      result: string,
      dispatchState: DispatchFeedbackState,
      targetSessionId = sessionId,
      dispatchId?: string,
    ) => {
      updateLiveSessionState(targetSessionId, (state) => ({
        ...state,
        pendingDispatches: [],
      }));

      try {
        if (isPlainChat) return;
        await enqueueDispatcherRun(targetSessionId, async (onEvent) => {
          await invoke<DispatcherAgentTurn>("dispatcher_continue_after_dispatch", {
            workspaceId: targetSessionId,
            projectPath,
            dispatchResult: result,
            dispatchState,
            dispatchId,
            onEvent,
          });
        });
      } catch (err) {
        console.error("dispatcher_continue_after_dispatch 失败:", err);
        updateLiveSessionState(targetSessionId, (state) => ({
          ...state,
          runError: `调度智能体继续执行失败：${toErrorMessage(err)}`,
        }));
      }
    },
    [enqueueDispatcherRun, isPlainChat, projectPath, sessionId, updateLiveSessionState],
  );

  const applyRuntimeState = useCallback(
    (state: DispatcherSessionRuntimeState) => {
      setMode(state.mode);
      setChecklist(state.checklist ?? null);
      setPlanInteraction(state.planInteraction ?? null);
      setActivePlanPath(state.activePlanPath ?? null);
    },
    [setActivePlanPath, setChecklist, setMode, setPlanInteraction],
  );

  return {
    enqueueDispatcherRun,
    sendUserMessage,
    continueWithResult,
    applyRuntimeState,
  };
}

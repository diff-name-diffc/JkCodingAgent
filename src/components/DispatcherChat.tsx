import {
  useState,
  useRef,
  useEffect,
  useCallback,
  useImperativeHandle,
  forwardRef,
  memo,
  useMemo,
} from "react";
import type { ReactNode } from "react";
import { invoke, Channel } from "@tauri-apps/api/core";
import {
  User,
  Sparkles,
  Send,
  X,
  FolderGit2,
  SearchCode,
  Wrench,
  Workflow,
  Settings2,
  PlugZap,
} from "lucide-react";
import type {
  AgentType,
  DispatchFeedbackState,
  DispatcherMessage,
  DispatcherAgentEvent,
  DispatcherAgentTurn,
  DispatcherSettings,
  ProjectMcpStatus,
  SubProcess,
} from "../types";
import { isImeComposing } from "../utils";
import { ToolActivityBubble, type ToolActivityItem } from "./ToolActivityBubble";
import { MarkdownRenderer } from "./markdown/MarkdownRenderer";
import {
  appendAssistantTextSegment,
  appendToolSummarySegment,
  type AssistantTurnSegment,
  buildDispatcherDisplayItems,
  finishLiveToolActivity,
  startLiveToolActivity,
} from "./dispatcherChatView";

// ── Dispatch Approval Dialog ─────────────────────────────────────────────────

interface DispatchApprovalProps {
  dispatchId: string;
  agent: AgentType;
  description: string;
  permissionMode: string;
  onApprove: (dispatchId: string, description: string) => void;
  onReject: (dispatchId: string) => void;
}

function DispatchApprovalDialog({
  dispatchId,
  agent,
  description,
  permissionMode,
  onApprove,
  onReject,
}: DispatchApprovalProps) {
  const [editedDescription, setEditedDescription] = useState(description);
  const meta = DISPATCH_AGENT_META[agent];

  return (
    <div style={styles.approvalOverlay}>
      <div style={styles.approvalDialog}>
        <div style={styles.approvalHeader}>
          <span style={styles.approvalIcon}>📋</span>
          <span style={styles.approvalTitle}>{meta.title}</span>
          <span style={styles.approvalAgentBadge}>{meta.badge}</span>
          <span style={styles.approvalBadge}>{permissionMode}</span>
        </div>
        <div style={styles.approvalHint}>{meta.hint}</div>
        <textarea
          style={styles.approvalTextarea}
          value={editedDescription}
          onChange={(e) => setEditedDescription(e.target.value)}
          rows={8}
        />
        <div style={styles.approvalActions}>
          <button style={styles.approvalRejectBtn} onClick={() => onReject(dispatchId)}>
            拒绝
          </button>
          <button
            style={styles.approvalApproveBtn}
            onClick={() => onApprove(dispatchId, editedDescription)}
          >
            ✓ 批准运行
          </button>
        </div>
      </div>
    </div>
  );
}

// ── Message Bubble ───────────────────────────────────────────────────────────

const UserMessageBubble = memo(function UserMessageBubble({
  message,
}: {
  message: DispatcherMessage;
}) {
  return (
    <div style={styles.messageBubbleWrap(true)}>
      <div style={styles.messageAvatar(true)}>
        <User size={15} color="#fff" />
      </div>
      <div style={styles.messageBubble(true)}>
        <div style={styles.messageText}>{message.content}</div>
      </div>
    </div>
  );
});

const AssistantTurnBubble = memo(function AssistantTurnBubble({
  segments,
  tools,
  workspaceId,
}: {
  segments: AssistantTurnSegment[];
  tools: ToolActivityItem[];
  workspaceId: string;
}) {
  const visibleSegments = segments.filter((segment) => segment.text.trim());
  if (visibleSegments.length === 0 && tools.length === 0) {
    return null;
  }

  return (
    <div style={styles.messageBubbleWrap(false)}>
      <div style={styles.messageAvatar(false)}>
        <Sparkles size={14} color="var(--accent)" />
      </div>
      <div style={styles.assistantTurnStack}>
        {tools.length > 0 && (
          <div style={styles.assistantTurnSection}>
            <ToolActivityBubble tools={tools} workspaceId={workspaceId} />
          </div>
        )}
        {visibleSegments.map((segment, index) => (
          <div key={`${segment.kind}-${segment.toolCallId ?? segment.toolName ?? index}`}>
            {segment.kind === "tool-summary" ? (
              <ToolSummaryBlock segment={segment} />
            ) : (
              <div style={styles.assistantTurnSection}>
                <div style={{ ...styles.messageBubble(false), ...styles.assistantReplyBubble }}>
                  <div style={styles.markdownBody}>
                    <MarkdownRenderer content={segment.text.trim()} variant="chat" />
                  </div>
                </div>
              </div>
            )}
          </div>
        ))}
      </div>
    </div>
  );
});

const ToolSummaryBlock = memo(function ToolSummaryBlock({
  segment,
}: {
  segment: AssistantTurnSegment;
}) {
  return (
    <div style={styles.assistantTurnSection}>
      <div style={styles.toolSummaryCard}>
        <div style={styles.toolSummaryHeader}>
          <span style={styles.toolSummaryBadge}>工具摘要</span>
          {segment.toolName && <span style={styles.toolSummaryName}>{segment.toolName}</span>}
          {segment.resultMode === "conservative_summary" && (
            <span style={styles.toolSummaryMode}>高保真压缩</span>
          )}
        </div>
        <div style={styles.markdownBody}>
          <MarkdownRenderer content={segment.text.trim()} variant="chat" />
        </div>
      </div>
    </div>
  );
});

const EmptyConversationLauncher = memo(function EmptyConversationLauncher({
  input,
  isLoading,
  autoApprove,
  inputRef,
  layoutMode,
  onChangeInput,
  onSelectQuickAction,
  onSend,
  onKeyDown,
  onOpenSettings,
  onOpenMcpStatus,
  onToggleAutoApprove,
  onCompositionStart,
  onCompositionEnd,
}: {
  input: string;
  isLoading: boolean;
  autoApprove: boolean;
  inputRef: React.RefObject<HTMLTextAreaElement | null>;
  layoutMode: "single" | "split";
  onChangeInput: (value: string) => void;
  onSelectQuickAction: (value: string) => void;
  onSend: () => void;
  onKeyDown: (e: React.KeyboardEvent) => void;
  onOpenSettings: () => void;
  onOpenMcpStatus: () => void;
  onToggleAutoApprove: () => void;
  onCompositionStart: () => void;
  onCompositionEnd: () => void;
}) {
  return (
    <div style={styles.emptyLauncherWrap}>
      <div style={styles.emptyLauncherHero}>
        <div style={styles.emptyLauncherKicker}>
          <span style={styles.emptyLauncherBadge}>
            <Sparkles size={13} />
            主调度智能体
          </span>
          <span style={styles.emptyLauncherMeta}>协调 Claude / Codex 子进程</span>
        </div>
        <h2 style={styles.emptyLauncherTitle}>今天想一起推进什么？</h2>
        <p style={styles.emptyLauncherSubtitle}>
          直接描述目标、粘贴报错，或者让我先读这个仓库。我会负责拆解任务、调用工具，并把执行进度持续回流到会话中。
        </p>
      </div>

      <div style={styles.emptyComposerDialog(layoutMode)}>
        <div style={styles.emptyComposerTopBar}>
          <div style={styles.emptyComposerPromptHint}>启动一轮新对话</div>
          <div style={styles.emptyComposerToolRow}>
            <button type="button" style={styles.emptyTopToolBtn} onClick={onOpenMcpStatus}>
              <PlugZap size={14} />
              MCP
            </button>
            <button type="button" style={styles.emptyTopToolBtn} onClick={onOpenSettings}>
              <Settings2 size={14} />
              设置
            </button>
            <button type="button" style={styles.emptyTopToolBtn} onClick={onToggleAutoApprove}>
              <Wrench size={14} />
              免确认 {autoApprove ? "开" : "关"}
            </button>
          </div>
        </div>

        <div style={styles.emptyComposerInputShell}>
          <textarea
            ref={inputRef}
            style={styles.emptyComposerTextarea}
            placeholder="例如：先审查这个仓库的前端架构，再给出重构方案并开始实现。"
            value={input}
            onChange={(e) => onChangeInput(e.target.value)}
            onCompositionStart={onCompositionStart}
            onCompositionEnd={onCompositionEnd}
            onKeyDown={onKeyDown}
            rows={4}
            disabled={isLoading}
          />
        </div>

        <div style={styles.emptyComposerActionRow}>
          {EMPTY_QUICK_ACTIONS.map((action) => (
            <button
              key={action.label}
              type="button"
              style={styles.emptyComposerActionBtn}
              onClick={() => onSelectQuickAction(action.prompt)}
            >
              <span style={styles.emptyComposerActionIcon}>{action.icon}</span>
              <span style={styles.emptyComposerActionLabel}>{action.label}</span>
            </button>
          ))}
        </div>

        <div style={styles.emptyComposerFooter}>
          <div style={styles.emptyComposerFootnote}>
            <span>Enter 发送</span>
            <span style={styles.emptyComposerFootnoteDot} />
            <span>Shift + Enter 换行</span>
            <span style={styles.emptyComposerFootnoteDot} />
            <span>支持直接贴入报错、日志或需求描述</span>
          </div>

          <div style={styles.emptyComposerBottomRow}>
            <div style={styles.emptyComposerSecondaryRow}>
              <button
                type="button"
                style={styles.emptySecondaryBtn}
                onClick={() =>
                  onSelectQuickAction(
                    "先浏览当前工作区的关键目录与项目结构，再告诉我应该从哪里开始。",
                  )
                }
              >
                浏览仓库
              </button>
              <button
                type="button"
                style={styles.emptySecondaryBtn}
                onClick={() =>
                  onSelectQuickAction("先检查当前仓库的未提交改动和最近提交，再总结上下文。")
                }
              >
                读取上下文
              </button>
            </div>

            <button
              type="button"
              style={{
                ...styles.emptyComposerSendBtn,
                opacity: input.trim() && !isLoading ? 1 : 0.45,
              }}
              onClick={onSend}
              disabled={!input.trim() || isLoading}
            >
              <span>开始对话</span>
              <Send size={15} />
            </button>
          </div>
        </div>
      </div>
    </div>
  );
});

function getMcpIndicatorState(
  mcpStatus: ProjectMcpStatus | null,
  mcpChecking: boolean,
): { color: string; label: string } {
  if (mcpChecking) {
    return { color: "#d97706", label: "检查中" };
  }
  if (!mcpStatus || mcpStatus.aggregate === "not_configured") {
    return { color: "var(--text-hint)", label: "未配置" };
  }
  if (mcpStatus.aggregate === "healthy") {
    return { color: "#1f9d55", label: "正常" };
  }
  return { color: "#dc2626", label: "异常" };
}

// ── DispatcherChat ───────────────────────────────────────────────────────────

interface DispatcherChatProps {
  sessionId: string;
  projectPath: string;
  mcpStatus: ProjectMcpStatus | null;
  mcpChecking: boolean;
  layoutMode?: "single" | "split";
  subProcesses: SubProcess[];
  onDispatchApproved: (
    dispatchId: string,
    agent: AgentType,
    description: string,
    permissionMode: string,
    sessionId: string,
  ) => void;
  onDispatchRejected: (dispatchId: string) => void;
  onDispatchContinue: (agent: AgentType, text: string, sessionId: string) => void;
  onDispatchExit: (agent: AgentType, reason: string, sessionId: string) => void;
  onOpenMcpStatus: () => void;
  onOpenSettings: () => void;
  onClosePanel?: () => void;
}

interface PendingDispatchApproval {
  dispatchId: string;
  agent: AgentType;
  description: string;
  permissionMode: string;
}

export interface DispatcherChatHandle {
  /** Inject dispatch result and continue the agent conversation */
  continueWithResult: (
    result: string,
    dispatchState: DispatchFeedbackState,
    targetSessionId?: string,
  ) => void;
}

export const DispatcherChat = forwardRef<DispatcherChatHandle, DispatcherChatProps>(
  function DispatcherChat(
    {
      sessionId,
      projectPath,
      mcpStatus,
      mcpChecking,
      layoutMode = "split",
      subProcesses: _subProcesses,
      onDispatchApproved,
      onDispatchRejected,
      onDispatchContinue,
      onDispatchExit,
      onOpenMcpStatus,
      onOpenSettings,
      onClosePanel,
    },
    ref,
  ) {
    const [messages, setMessages] = useState<DispatcherMessage[]>([]);
    const [input, setInput] = useState("");
    const [isLoading, setIsLoading] = useState(false);
    const [streamingSegments, setStreamingSegments] = useState<AssistantTurnSegment[]>([]);
    const [liveToolCalls, setLiveToolCalls] = useState<ToolActivityItem[]>([]);
    const [pendingDispatches, setPendingDispatches] = useState<PendingDispatchApproval[]>([]);
    const [autoApprove, setAutoApprove] = useState(false);

    const messagesEndRef = useRef<HTMLDivElement>(null);
    const inputRef = useRef<HTMLTextAreaElement>(null);
    const inputComposingRef = useRef(false);
    const currentSessionIdRef = useRef(sessionId);
    currentSessionIdRef.current = sessionId;
    const activeRunRef = useRef(0);
    const historyLoadRef = useRef(0);
    const runQueuesRef = useRef<Map<string, Promise<void>>>(new Map());
    const autoApproveRef = useRef(autoApprove);
    autoApproveRef.current = autoApprove;
    const onDispatchApprovedRef = useRef(onDispatchApproved);
    onDispatchApprovedRef.current = onDispatchApproved;
    const onDispatchContinueRef = useRef(onDispatchContinue);
    onDispatchContinueRef.current = onDispatchContinue;
    const onDispatchExitRef = useRef(onDispatchExit);
    onDispatchExitRef.current = onDispatchExit;
    const displayItems = useMemo(() => buildDispatcherDisplayItems(messages), [messages]);
    const currentPendingDispatch = pendingDispatches[0] ?? null;
    const mcpIndicator = getMcpIndicatorState(mcpStatus, mcpChecking);

    // Load settings (for auto-approve flag)
    useEffect(() => {
      invoke<DispatcherSettings | null>("dispatcher_get_settings")
        .then((s) => {
          if (s) setAutoApprove(s.autoApproveDispatch);
        })
        .catch(console.error);
    }, []);

    // Load history for the active session only. Late responses from a previous
    // session must not overwrite the currently selected session.
    useEffect(() => {
      const loadId = ++historyLoadRef.current;
      activeRunRef.current += 1;
      setMessages([]);
      setIsLoading(false);
      setStreamingSegments([]);
      setLiveToolCalls([]);
      setPendingDispatches([]);

      invoke<DispatcherMessage[]>("dispatcher_list_messages", {
        workspaceId: sessionId,
      })
        .then((loaded) => {
          if (currentSessionIdRef.current !== sessionId || historyLoadRef.current !== loadId)
            return;
          setMessages(loaded.filter((message) => message.workspaceId === sessionId));
        })
        .catch(console.error);
    }, [sessionId]);

    // Auto-scroll
    useEffect(() => {
      messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
    }, [messages, streamingSegments]);

    const createEventChannel = useCallback((targetSessionId: string, runId: number) => {
      const onEvent = new Channel<DispatcherAgentEvent>();
      onEvent.onmessage = (event) => {
        const isCurrentRun =
          currentSessionIdRef.current === targetSessionId && activeRunRef.current === runId;
        switch (event.event) {
          case "started":
          case "assistantStarted":
            break;
          case "userMessage":
            if (!isCurrentRun || event.data.message.workspaceId !== targetSessionId) return;
            setMessages((prev) => [...prev, event.data.message]);
            break;
          case "assistantDelta":
            if (!isCurrentRun) return;
            setStreamingSegments((prev) => appendAssistantTextSegment(prev, event.data.delta));
            break;
          case "assistantMessage":
            if (!isCurrentRun || event.data.message.workspaceId !== targetSessionId) return;
            setStreamingSegments((prev) =>
              prev.filter((segment) => segment.kind === "tool-summary"),
            );
            setMessages((prev) => [...prev, event.data.message]);
            break;
          case "toolStarted":
            if (!isCurrentRun) return;
            setLiveToolCalls((prev) => startLiveToolActivity(prev, event.data));
            break;
          case "toolSummaryStarted":
            if (!isCurrentRun) return;
            break;
          case "toolSummaryDelta":
            if (!isCurrentRun) return;
            setStreamingSegments((prev) => appendToolSummarySegment(prev, event.data));
            break;
          case "toolFinished":
            if (!isCurrentRun) return;
            setLiveToolCalls((prev) => finishLiveToolActivity(prev, event.data));
            break;
          case "dispatchProposed": {
            const { dispatchId, agent, description, permissionMode } = event.data;
            if (autoApproveRef.current) {
              onDispatchApprovedRef.current(
                dispatchId,
                agent,
                description,
                permissionMode,
                targetSessionId,
              );
            } else if (isCurrentRun) {
              setPendingDispatches((prev) => [
                ...prev,
                { dispatchId, agent, description, permissionMode },
              ]);
            }
            break;
          }
          case "dispatchContinue": {
            onDispatchContinueRef.current(event.data.agent, event.data.text, targetSessionId);
            break;
          }
          case "dispatchExit": {
            onDispatchExitRef.current(event.data.agent, event.data.reason, targetSessionId);
            break;
          }
          case "finished":
            if (!isCurrentRun) return;
            setMessages(
              event.data.messages.filter((message) => message.workspaceId === targetSessionId),
            );
            setIsLoading(false);
            setStreamingSegments([]);
            setLiveToolCalls([]);
            break;
        }
      };
      return onEvent;
    }, []);

    const enqueueDispatcherRun = useCallback(
      async (
        targetSessionId: string,
        runner: (onEvent: Channel<DispatcherAgentEvent>) => Promise<void>,
      ) => {
        const previous = runQueuesRef.current.get(targetSessionId) ?? Promise.resolve();
        const queued = previous
          .catch(() => undefined)
          .then(async () => {
            const isCurrentSession = currentSessionIdRef.current === targetSessionId;
            const runId = isCurrentSession ? ++activeRunRef.current : activeRunRef.current;

            if (isCurrentSession) {
              setIsLoading(true);
              setStreamingSegments([]);
              setLiveToolCalls([]);
            }

            const onEvent = createEventChannel(targetSessionId, runId);

            try {
              await runner(onEvent);
            } finally {
              if (
                currentSessionIdRef.current === targetSessionId &&
                activeRunRef.current === runId
              ) {
                setIsLoading(false);
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
      [createEventChannel],
    );

    const handleSend = useCallback(async () => {
      const text = input.trim();
      if (!text || isLoading) return;

      setInput("");
      setPendingDispatches([]);

      const targetSessionId = sessionId;

      try {
        await enqueueDispatcherRun(targetSessionId, async (onEvent) => {
          await invoke<DispatcherAgentTurn>("dispatcher_send_message", {
            workspaceId: targetSessionId,
            projectPath,
            content: text,
            onEvent,
          });
        });
      } catch (err) {
        console.error("dispatcher_send_message 失败:", err);
      }
    }, [enqueueDispatcherRun, input, isLoading, projectPath, sessionId]);

    // Expose continueWithResult to parent via ref
    useImperativeHandle(
      ref,
      () => ({
        continueWithResult: async (
          result: string,
          dispatchState: DispatchFeedbackState,
          targetSessionId = sessionId,
        ) => {
          if (currentSessionIdRef.current === targetSessionId) {
            setPendingDispatches([]);
          }

          try {
            await enqueueDispatcherRun(targetSessionId, async (onEvent) => {
              await invoke<DispatcherAgentTurn>("dispatcher_continue_after_dispatch", {
                workspaceId: targetSessionId,
                projectPath,
                dispatchResult: result,
                dispatchState,
                onEvent,
              });
            });
          } catch (err) {
            console.error("dispatcher_continue_after_dispatch 失败:", err);
          }
        },
      }),
      [enqueueDispatcherRun, projectPath, sessionId],
    );

    const handleKeyDown = useCallback(
      (e: React.KeyboardEvent) => {
        if (inputComposingRef.current || isImeComposing(e)) {
          return;
        }
        if (e.key === "Enter" && !e.shiftKey) {
          e.preventDefault();
          handleSend();
        }
      },
      [handleSend],
    );

    const handleApproveDispatch = useCallback(
      (dispatchId: string, description: string) => {
        const agent = currentPendingDispatch?.agent ?? "claude";
        const pm = currentPendingDispatch?.permissionMode ?? "full_access";
        setPendingDispatches((prev) => prev.slice(1));
        onDispatchApproved(dispatchId, agent, description, pm, sessionId);
      },
      [currentPendingDispatch, onDispatchApproved, sessionId],
    );

    const handleRejectDispatch = useCallback(
      (dispatchId: string) => {
        setPendingDispatches((prev) => prev.slice(1));
        onDispatchRejected(dispatchId);
      },
      [onDispatchRejected],
    );

    const handleToggleAutoApprove = useCallback(async () => {
      const next = !autoApprove;
      setAutoApprove(next);
      try {
        const saved = await invoke<DispatcherSettings>("dispatcher_set_auto_approve_dispatch", {
          autoApproveDispatch: next,
        });
        setAutoApprove(saved.autoApproveDispatch);
      } catch (err) {
        setAutoApprove(!next);
        console.error("dispatcher_set_auto_approve_dispatch 失败:", err);
      }
    }, [autoApprove]);

    const handleClearHistory = useCallback(async () => {
      try {
        await invoke("dispatcher_clear_messages", {
          workspaceId: sessionId,
        });
        setMessages([]);
      } catch (err) {
        console.error("清空消息失败:", err);
      }
    }, [sessionId]);

    const handleApplyStarterPrompt = useCallback((prompt: string) => {
      setInput(prompt);
      inputRef.current?.focus();
    }, []);

    const hasLiveSegments = streamingSegments.some((segment) => segment.text.trim());
    const isEmpty = messages.length === 0 && !hasLiveSegments && liveToolCalls.length === 0;

    return (
      <div style={styles.container}>
        {/* Header */}
        <div style={styles.header}>
          <div style={styles.headerLeft}>
            <span style={styles.headerIcon}>🤖</span>
            <span style={styles.headerTitle}>调度智能体</span>
            {isLoading && <span style={styles.thinkingDot} />}
          </div>
          <div style={styles.headerRight}>
            <button
              style={{
                ...styles.headerBtn,
                ...(autoApprove ? styles.headerBtnActive : {}),
              }}
              onClick={handleToggleAutoApprove}
              title="开启后，调度给 Claude 或 Codex 子任务时不再弹出审查确认"
            >
              免确认 {autoApprove ? "开" : "关"}
            </button>
            <button
              style={styles.headerBtn}
              onClick={onOpenMcpStatus}
              title={`项目级 MCP 状态：${mcpIndicator.label}`}
            >
              <span
                style={{
                  ...styles.headerSignal,
                  background: mcpIndicator.color,
                  boxShadow: `0 0 0 3px ${mcpIndicator.color}22`,
                }}
              />
              MCP
            </button>
            {messages.length > 0 && (
              <button style={styles.headerBtn} onClick={handleClearHistory}>
                清空
              </button>
            )}
            <button style={styles.headerBtn} onClick={onOpenSettings}>
              ⚙ 设置
            </button>
            {onClosePanel && (
              <button
                style={styles.headerBtn}
                onClick={onClosePanel}
                title="关闭会话面板"
                aria-label="关闭会话面板"
              >
                <X size={14} />
              </button>
            )}
          </div>
        </div>

        {/* Messages */}
        <div style={styles.messageList}>
          {isEmpty && (
            <EmptyConversationLauncher
              input={input}
              isLoading={isLoading}
              autoApprove={autoApprove}
              inputRef={inputRef}
              layoutMode={layoutMode}
              onChangeInput={setInput}
              onSelectQuickAction={handleApplyStarterPrompt}
              onSend={handleSend}
              onKeyDown={handleKeyDown}
              onOpenSettings={onOpenSettings}
              onOpenMcpStatus={onOpenMcpStatus}
              onToggleAutoApprove={handleToggleAutoApprove}
              onCompositionStart={() => {
                inputComposingRef.current = true;
              }}
              onCompositionEnd={() => {
                inputComposingRef.current = false;
              }}
            />
          )}
          {displayItems.map((item) =>
            item.kind === "user" ? (
              <UserMessageBubble key={item.id} message={item.message} />
            ) : (
              <AssistantTurnBubble
                key={item.id}
                segments={item.turn.segments}
                tools={item.turn.tools}
                workspaceId={sessionId}
              />
            ),
          )}
          {(hasLiveSegments || liveToolCalls.length > 0) && (
            <AssistantTurnBubble
              segments={streamingSegments}
              tools={liveToolCalls}
              workspaceId={sessionId}
            />
          )}
          <div ref={messagesEndRef} />
        </div>

        {/* Input */}
        {!isEmpty && (
          <div style={styles.inputArea}>
            <textarea
              ref={inputRef}
              style={styles.inputTextarea}
              placeholder="给调度智能体发送消息..."
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onCompositionStart={() => {
                inputComposingRef.current = true;
              }}
              onCompositionEnd={() => {
                inputComposingRef.current = false;
              }}
              onKeyDown={handleKeyDown}
              rows={1}
              disabled={isLoading}
            />
            <button
              style={{ ...styles.sendBtn, opacity: input.trim() && !isLoading ? 1 : 0.5 }}
              onClick={handleSend}
              disabled={!input.trim() || isLoading}
            >
              <Send size={16} color="#fff" />
            </button>
          </div>
        )}

        {/* Dispatch approval overlay */}
        {currentPendingDispatch && (
          <DispatchApprovalDialog
            dispatchId={currentPendingDispatch.dispatchId}
            agent={currentPendingDispatch.agent}
            description={currentPendingDispatch.description}
            permissionMode={currentPendingDispatch.permissionMode}
            onApprove={handleApproveDispatch}
            onReject={handleRejectDispatch}
          />
        )}
      </div>
    );
  },
);

const DISPATCH_AGENT_META: Record<AgentType, { title: string; badge: string; hint: string }> = {
  claude: {
    title: "Claude 子任务审查",
    badge: "Claude",
    hint: "Claude 更快，适合新功能、算法试验和问题探索。",
  },
  codex: {
    title: "Codex 子任务审查",
    badge: "Codex",
    hint: "Codex 更慢但更仔细，适合重构、结构整理和高风险修改。",
  },
};

const EMPTY_QUICK_ACTIONS: Array<{
  label: string;
  prompt: string;
  icon: ReactNode;
}> = [
  {
    label: "审查项目结构",
    prompt: "先审查这个项目的整体架构、关键模块和潜在风险，再给我一个简洁的分析结论。",
    icon: <SearchCode size={14} />,
  },
  {
    label: "查看最近改动",
    prompt: "先查看这个项目最近的 Git 变更和当前工作区状态，再总结我现在最该关注的内容。",
    icon: <FolderGit2 size={14} />,
  },
  {
    label: "制定实现计划",
    prompt: "请先理解当前代码库，再针对我要做的需求给出一个清晰、可执行的实现计划。",
    icon: <Workflow size={14} />,
  },
  {
    label: "排查构建问题",
    prompt: "请先检查当前项目是否能正常构建、测试或 lint，并定位阻塞问题。",
    icon: <Wrench size={14} />,
  },
];

// ── Styles ───────────────────────────────────────────────────────────────────

const styles = {
  container: {
    display: "flex",
    flexDirection: "column" as const,
    height: "100%",
    background:
      "radial-gradient(circle at top right, color-mix(in srgb, var(--accent) 10%, transparent), transparent 26%), linear-gradient(180deg, var(--bg-panel), color-mix(in srgb, var(--bg-panel) 78%, var(--bg-subtle)))",
  },
  header: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    padding: "12px 18px",
    borderBottom: "1px solid var(--border-dim)",
    background: "color-mix(in srgb, var(--bg-card) 68%, transparent)",
    backdropFilter: "blur(16px)",
    WebkitBackdropFilter: "blur(16px)",
    WebkitAppRegion: "drag" as const,
    flexShrink: 0,
  },
  headerLeft: {
    display: "flex",
    alignItems: "center",
    gap: "8px",
  },
  headerIcon: { fontSize: "16px" },
  headerTitle: {
    fontSize: "13.5px",
    fontWeight: 700,
    letterSpacing: "-0.01em",
    color: "var(--text-primary)",
  },
  thinkingDot: {
    width: "8px",
    height: "8px",
    borderRadius: "50%",
    background: "var(--accent)",
    animation: "pulse 1.5s ease-in-out infinite",
  },
  headerRight: {
    display: "flex",
    alignItems: "center",
    gap: "4px",
    WebkitAppRegion: "no-drag" as const,
  },
  headerBtn: {
    padding: "6px 10px",
    fontSize: "11px",
    display: "inline-flex",
    alignItems: "center",
    gap: "6px",
    background: "color-mix(in srgb, var(--bg-card) 82%, transparent)",
    border: "1px solid var(--border-dim)",
    borderRadius: "999px",
    color: "var(--text-secondary)",
    cursor: "pointer",
  },
  headerSignal: {
    width: "8px",
    height: "8px",
    borderRadius: "999px",
    flexShrink: 0,
  },
  headerBtnActive: {
    background: "var(--accent-subtle)",
    borderColor: "var(--accent)",
    color: "var(--accent)",
  },
  messageList: {
    flex: 1,
    overflowY: "auto" as const,
    padding: "22px 20px 18px",
    display: "flex",
    flexDirection: "column" as const,
    gap: "18px",
  },
  emptyLauncherWrap: {
    display: "flex",
    flexDirection: "column" as const,
    alignItems: "center",
    justifyContent: "center",
    flex: 1,
    width: "100%",
    padding: "28px 12px 44px",
    boxSizing: "border-box" as const,
  },
  emptyLauncherHero: {
    width: "100%",
    maxWidth: "780px",
    display: "flex",
    flexDirection: "column" as const,
    alignItems: "center",
    gap: "14px",
    marginBottom: "26px",
    textAlign: "center" as const,
  },
  emptyLauncherKicker: {
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    gap: "10px",
    flexWrap: "wrap" as const,
  },
  emptyLauncherBadge: {
    display: "inline-flex",
    alignItems: "center",
    gap: "7px",
    padding: "7px 12px",
    borderRadius: "999px",
    border: "1px solid color-mix(in srgb, var(--accent) 16%, var(--border-dim))",
    background: "color-mix(in srgb, var(--bg-card) 76%, transparent)",
    color: "var(--text-secondary)",
    fontSize: "12px",
    fontWeight: 600,
    boxShadow: "0 12px 30px rgba(15, 23, 42, 0.04)",
  },
  emptyLauncherMeta: {
    fontSize: "12.5px",
    color: "var(--text-muted)",
    letterSpacing: "0.01em",
  },
  emptyLauncherTitle: {
    margin: 0,
    fontSize: "34px",
    lineHeight: 1.04,
    letterSpacing: "-0.04em",
    fontWeight: 700,
    color: "var(--text-primary)",
  },
  emptyLauncherSubtitle: {
    margin: 0,
    fontSize: "15px",
    color: "var(--text-secondary)",
    maxWidth: "700px",
    lineHeight: "1.72",
  },
  emptyComposerDialog: (layoutMode: "single" | "split") => ({
    width: "100%",
    maxWidth: layoutMode === "single" ? "980px" : "860px",
    display: "flex",
    flexDirection: "column" as const,
    gap: "18px",
    padding: "18px",
    borderRadius: "30px",
    border: "1px solid color-mix(in srgb, var(--accent) 10%, var(--border-dim))",
    background:
      "linear-gradient(180deg, color-mix(in srgb, var(--bg-card) 94%, transparent), color-mix(in srgb, var(--bg-subtle) 82%, transparent))",
    boxShadow: "0 36px 100px rgba(15, 23, 42, 0.09)",
    backdropFilter: "blur(22px)",
    WebkitBackdropFilter: "blur(22px)",
    boxSizing: "border-box" as const,
  }),
  emptyComposerTopBar: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: "12px",
    flexWrap: "wrap" as const,
  },
  emptyComposerPromptHint: {
    fontSize: "12px",
    fontWeight: 700,
    letterSpacing: "0.06em",
    textTransform: "uppercase" as const,
    color: "var(--text-hint)",
  },
  emptyComposerToolRow: {
    display: "flex",
    alignItems: "center",
    gap: "8px",
    flexWrap: "wrap" as const,
  },
  emptyTopToolBtn: {
    display: "inline-flex",
    alignItems: "center",
    gap: "6px",
    height: "34px",
    padding: "0 12px",
    borderRadius: "999px",
    border: "1px solid var(--border-dim)",
    background: "color-mix(in srgb, var(--bg-card) 74%, transparent)",
    color: "var(--text-secondary)",
    fontSize: "12px",
    fontWeight: 600,
    cursor: "pointer",
  },
  emptyComposerInputShell: {
    borderRadius: "24px",
    border: "1px solid color-mix(in srgb, var(--accent) 8%, var(--border-dim))",
    background:
      "linear-gradient(180deg, color-mix(in srgb, var(--bg-card) 96%, transparent), color-mix(in srgb, var(--bg-subtle) 72%, transparent))",
    boxShadow: "inset 0 1px 0 rgba(255,255,255,0.2)",
  },
  emptyComposerTextarea: {
    width: "100%",
    minHeight: "150px",
    padding: "20px 22px",
    border: "none",
    outline: "none",
    resize: "none" as const,
    background: "transparent",
    color: "var(--text-primary)",
    fontSize: "19px",
    lineHeight: "1.7",
    fontFamily: "var(--font-ui)",
    boxSizing: "border-box" as const,
  },
  emptyComposerActionRow: {
    display: "flex",
    flexWrap: "wrap" as const,
    gap: "10px",
  },
  emptyComposerActionBtn: {
    display: "inline-flex",
    alignItems: "center",
    gap: "8px",
    minHeight: "38px",
    padding: "8px 13px",
    borderRadius: "999px",
    border: "1px solid color-mix(in srgb, var(--accent) 8%, var(--border-dim))",
    background: "color-mix(in srgb, var(--bg-card) 68%, transparent)",
    color: "var(--text-secondary)",
    fontSize: "12.5px",
    fontWeight: 600,
    cursor: "pointer",
  },
  emptyComposerActionIcon: {
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    width: "18px",
    height: "18px",
    color: "var(--text-primary)",
  },
  emptyComposerActionLabel: {
    whiteSpace: "nowrap" as const,
  },
  emptyComposerFooter: {
    display: "flex",
    flexDirection: "column" as const,
    gap: "14px",
    paddingTop: "2px",
  },
  emptyComposerFootnote: {
    display: "flex",
    alignItems: "center",
    gap: "10px",
    flexWrap: "wrap" as const,
    color: "var(--text-muted)",
    fontSize: "12px",
    lineHeight: 1.6,
  },
  emptyComposerFootnoteDot: {
    width: "4px",
    height: "4px",
    borderRadius: "999px",
    background: "var(--text-hint)",
    flexShrink: 0,
  },
  emptyComposerBottomRow: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: "12px",
    flexWrap: "wrap" as const,
  },
  emptyComposerSecondaryRow: {
    display: "flex",
    alignItems: "center",
    gap: "10px",
    flexWrap: "wrap" as const,
  },
  emptySecondaryBtn: {
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    padding: "8px 12px",
    borderRadius: "999px",
    border: "1px solid var(--border-dim)",
    background: "transparent",
    color: "var(--text-secondary)",
    fontSize: "12px",
    fontWeight: 600,
    cursor: "pointer",
  },
  emptyComposerSendBtn: {
    display: "inline-flex",
    alignItems: "center",
    gap: "8px",
    height: "44px",
    padding: "0 18px",
    borderRadius: "999px",
    border: "none",
    background:
      "linear-gradient(135deg, var(--accent), color-mix(in srgb, var(--accent) 74%, white))",
    color: "#fff",
    fontSize: "13px",
    fontWeight: 700,
    cursor: "pointer",
    boxShadow: "0 18px 28px -16px color-mix(in srgb, var(--accent) 60%, transparent)",
  },
  messageBubbleWrap: (isUser: boolean) => ({
    display: "flex",
    flexDirection: isUser ? ("row-reverse" as const) : ("row" as const),
    gap: "14px",
    alignItems: "flex-start",
    marginBottom: "2px",
  }),
  messageAvatar: (isUser: boolean) => ({
    width: "34px",
    height: "34px",
    borderRadius: "12px",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    background: isUser
      ? "linear-gradient(135deg, var(--accent), color-mix(in srgb, var(--accent) 72%, white))"
      : "linear-gradient(135deg, color-mix(in srgb, var(--bg-card) 92%, transparent), color-mix(in srgb, var(--bg-subtle) 84%, transparent))",
    border: "1px solid var(--border-dim)",
    boxShadow: isUser
      ? "0 8px 18px -8px color-mix(in srgb, var(--accent) 48%, transparent)"
      : "0 8px 18px -10px rgba(0,0,0,0.12)",
    flexShrink: 0,
    marginTop: "2px",
  }),
  messageBubble: (isUser: boolean) => ({
    maxWidth: "100%",
    padding: "14px 16px",
    borderRadius: isUser ? "22px 22px 8px 22px" : "22px 22px 22px 8px",
    background: isUser
      ? "linear-gradient(135deg, var(--accent), color-mix(in srgb, var(--accent) 82%, white))"
      : "linear-gradient(180deg, color-mix(in srgb, var(--bg-card) 96%, transparent), color-mix(in srgb, var(--bg-subtle) 82%, transparent))",
    color: isUser ? "#fff" : "var(--text-primary)",
    border: isUser ? "none" : "1px solid color-mix(in srgb, var(--accent) 10%, var(--border-dim))",
    boxShadow: isUser
      ? "0 16px 28px -18px color-mix(in srgb, var(--accent) 50%, transparent)"
      : "0 16px 32px rgba(15, 23, 42, 0.05)",
    fontSize: "14px",
    lineHeight: "1.7",
    wordBreak: "break-word" as const,
    userSelect: "text" as const,
    WebkitUserSelect: "text" as const,
  }),
  messageText: {
    whiteSpace: "pre-wrap" as const,
    userSelect: "text" as const,
    WebkitUserSelect: "text" as const,
    fontFamily: "var(--font-ui)",
  },
  markdownBody: {
    fontSize: "14px",
    lineHeight: "1.72",
    userSelect: "text" as const,
    WebkitUserSelect: "text" as const,
    fontFamily: "var(--font-ui)",
  },
  assistantTurnStack: {
    maxWidth: "88%",
    display: "flex",
    flexDirection: "column" as const,
    gap: "12px",
    minWidth: 0,
  },
  assistantTurnSection: {
    width: "100%",
    minWidth: 0,
  },
  assistantReplyBubble: {
    background:
      "linear-gradient(180deg, color-mix(in srgb, var(--bg-card) 96%, transparent), color-mix(in srgb, var(--bg-subtle) 82%, transparent))",
  },
  toolSummaryCard: {
    width: "100%",
    padding: "14px 16px",
    borderRadius: "22px 22px 22px 8px",
    border: "1px solid color-mix(in srgb, var(--warning, #d97706) 22%, var(--border-dim))",
    background:
      "linear-gradient(180deg, rgba(217,119,6,0.08), color-mix(in srgb, var(--bg-card) 94%, transparent))",
    boxShadow: "0 16px 32px rgba(15, 23, 42, 0.05)",
  },
  toolSummaryHeader: {
    display: "flex",
    alignItems: "center",
    gap: "8px",
    flexWrap: "wrap" as const,
    marginBottom: "10px",
  },
  toolSummaryBadge: {
    display: "inline-flex",
    alignItems: "center",
    padding: "4px 9px",
    borderRadius: "999px",
    background: "rgba(217,119,6,0.12)",
    color: "var(--warning, #d97706)",
    fontSize: "11px",
    fontWeight: 700,
    letterSpacing: "0.02em",
  },
  toolSummaryName: {
    fontSize: "12px",
    fontWeight: 600,
    color: "var(--text-primary)",
  },
  toolSummaryMode: {
    fontSize: "11px",
    color: "var(--text-secondary)",
  },
  inputArea: {
    display: "flex",
    alignItems: "flex-end",
    gap: "12px",
    padding: "14px 18px 18px",
    background:
      "linear-gradient(180deg, color-mix(in srgb, var(--bg-panel) 0%, transparent), color-mix(in srgb, var(--bg-card) 40%, transparent))",
    flexShrink: 0,
  },
  inputTextarea: {
    flex: 1,
    padding: "14px 16px",
    fontSize: "14px",
    lineHeight: "1.6",
    background: "color-mix(in srgb, var(--bg-card) 92%, transparent)",
    border: "1px solid color-mix(in srgb, var(--accent) 10%, var(--border-medium))",
    borderRadius: "18px",
    color: "var(--text-primary)",
    resize: "none" as const,
    outline: "none",
    fontFamily: "var(--font-ui)",
    boxShadow: "0 12px 30px rgba(15, 23, 42, 0.05)",
    transition: "border-color 0.2s, box-shadow 0.2s",
  },
  sendBtn: {
    width: "44px",
    height: "44px",
    borderRadius: "16px",
    background:
      "linear-gradient(135deg, var(--accent), color-mix(in srgb, var(--accent) 74%, white))",
    color: "#fff",
    border: "none",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    cursor: "pointer",
    transition: "opacity 0.2s, transform 0.1s",
    boxShadow: "0 18px 28px -16px color-mix(in srgb, var(--accent) 60%, transparent)",
  },
  // ── Approval dialog styles ──
  approvalOverlay: {
    position: "absolute" as const,
    inset: 0,
    background: "rgba(0,0,0,0.4)",
    backdropFilter: "blur(4px)",
    WebkitBackdropFilter: "blur(4px)",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    zIndex: 100,
  },
  approvalDialog: {
    width: "90%",
    maxWidth: "520px",
    background: "var(--bg-card)",
    border: "1px solid var(--border-medium)",
    borderRadius: "16px",
    padding: "20px",
    display: "flex",
    flexDirection: "column" as const,
    gap: "16px",
    boxShadow: "0 10px 40px rgba(0,0,0,0.2)",
  },
  approvalHeader: {
    display: "flex",
    alignItems: "center",
    gap: "10px",
  },
  approvalIcon: { fontSize: "18px", color: "var(--accent)" },
  approvalTitle: {
    fontSize: "14.5px",
    fontWeight: 600,
    color: "var(--text-primary)",
    flex: 1,
  },
  approvalAgentBadge: {
    fontSize: "11px",
    padding: "3px 8px",
    borderRadius: "999px",
    background: "color-mix(in srgb, var(--accent) 12%, transparent)",
    color: "var(--accent)",
    border: "1px solid color-mix(in srgb, var(--accent) 22%, transparent)",
    fontWeight: 700,
  },
  approvalBadge: {
    fontSize: "11px",
    padding: "3px 8px",
    borderRadius: "6px",
    background: "var(--bg-subtle)",
    color: "var(--text-secondary)",
    fontFamily: "var(--font-mono)",
    fontWeight: 500,
  },
  approvalHint: {
    fontSize: "12px",
    lineHeight: "1.6",
    color: "var(--text-secondary)",
  },
  approvalTextarea: {
    width: "100%",
    padding: "12px",
    fontSize: "13px",
    lineHeight: "1.6",
    background: "var(--bg-input)",
    border: "1px solid var(--border-medium)",
    borderRadius: "10px",
    color: "var(--text-primary)",
    resize: "vertical" as const,
    outline: "none",
    fontFamily: "var(--font-mono)",
    boxSizing: "border-box" as const,
    boxShadow: "0 2px 6px rgba(0,0,0,0.02) inset",
  },
  approvalActions: {
    display: "flex",
    justifyContent: "flex-end",
    gap: "10px",
    marginTop: "4px",
  },
  approvalRejectBtn: {
    padding: "8px 18px",
    fontSize: "12.5px",
    background: "var(--bg-subtle)",
    border: "1px solid var(--border-medium)",
    borderRadius: "8px",
    color: "var(--text-secondary)",
    cursor: "pointer",
    fontWeight: 500,
  },
  approvalApproveBtn: {
    padding: "8px 18px",
    fontSize: "12.5px",
    background: "var(--accent)",
    color: "#fff",
    border: "none",
    borderRadius: "8px",
    cursor: "pointer",
    fontWeight: 600,
    boxShadow: "0 2px 6px color-mix(in srgb, var(--accent) 40%, transparent)",
  },
};

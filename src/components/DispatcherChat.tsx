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
import { invoke, Channel } from "@tauri-apps/api/core";
import { User, Sparkles, Send } from "lucide-react";
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
  responseText,
  tools,
}: {
  responseText: string;
  tools: ToolActivityItem[];
}) {
  const trimmedResponse = responseText.trim();
  if (!trimmedResponse && tools.length === 0) {
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
            <ToolActivityBubble tools={tools} />
          </div>
        )}
        {trimmedResponse && (
          <div style={styles.assistantTurnSection}>
            <div style={{ ...styles.messageBubble(false), ...styles.assistantReplyBubble }}>
              <div style={styles.markdownBody}>
                <MarkdownRenderer content={trimmedResponse} variant="chat" />
              </div>
            </div>
          </div>
        )}
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
      subProcesses: _subProcesses,
      onDispatchApproved,
      onDispatchRejected,
      onDispatchContinue,
      onDispatchExit,
      onOpenMcpStatus,
      onOpenSettings,
    },
    ref,
  ) {
    const [messages, setMessages] = useState<DispatcherMessage[]>([]);
    const [input, setInput] = useState("");
    const [isLoading, setIsLoading] = useState(false);
    const [streamingContent, setStreamingContent] = useState("");
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
      setStreamingContent("");
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
    }, [messages, streamingContent]);

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
            setStreamingContent((prev) => prev + event.data.delta);
            break;
          case "assistantMessage":
            if (!isCurrentRun || event.data.message.workspaceId !== targetSessionId) return;
            setStreamingContent("");
            setMessages((prev) => [...prev, event.data.message]);
            break;
          case "toolStarted":
            if (!isCurrentRun) return;
            setLiveToolCalls((prev) => startLiveToolActivity(prev, event.data));
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
            setStreamingContent("");
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
              setStreamingContent("");
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

    const isEmpty = messages.length === 0 && !streamingContent && liveToolCalls.length === 0;

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
          </div>
        </div>

        {/* Messages */}
        <div style={styles.messageList}>
          {isEmpty && (
            <div style={styles.emptyState}>
              <div style={styles.emptyIcon}>🤖</div>
              <div style={styles.emptyTitle}>调度智能体</div>
              <div style={styles.emptySubtitle}>
                告诉我你想完成什么，我会自动规划，并在 Claude 与 Codex
                之间选择合适的子进程推进编码任务
              </div>
              <div style={styles.emptyMeta}>
                Claude 更快，适合新功能、算法与探索；Codex 更稳，适合重构与收口。
              </div>
            </div>
          )}
          {displayItems.map((item) =>
            item.kind === "user" ? (
              <UserMessageBubble key={item.id} message={item.message} />
            ) : (
              <AssistantTurnBubble
                key={item.id}
                responseText={item.turn.responseParts.join("\n\n")}
                tools={item.turn.tools}
              />
            ),
          )}
          {(streamingContent.trim() || liveToolCalls.length > 0) && (
            <AssistantTurnBubble responseText={streamingContent} tools={liveToolCalls} />
          )}
          <div ref={messagesEndRef} />
        </div>

        {/* Input */}
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
  emptyState: {
    display: "flex",
    flexDirection: "column" as const,
    alignItems: "center",
    justifyContent: "center",
    flex: 1,
    gap: "10px",
    margin: "32px auto",
    padding: "36px 32px",
    maxWidth: "520px",
    borderRadius: "28px",
    border: "1px solid color-mix(in srgb, var(--accent) 12%, var(--border-dim))",
    background:
      "linear-gradient(135deg, color-mix(in srgb, var(--accent) 10%, transparent), transparent 55%), color-mix(in srgb, var(--bg-card) 92%, transparent)",
    boxShadow: "0 18px 60px rgba(15, 23, 42, 0.06)",
  },
  emptyIcon: { fontSize: "48px" },
  emptyTitle: {
    fontSize: "22px",
    fontWeight: 700,
    letterSpacing: "-0.03em",
    color: "var(--text-primary)",
  },
  emptySubtitle: {
    fontSize: "14px",
    color: "var(--text-secondary)",
    textAlign: "center" as const,
    maxWidth: "420px",
    lineHeight: "1.7",
  },
  emptyMeta: {
    fontSize: "12px",
    color: "var(--text-hint)",
    textAlign: "center" as const,
    maxWidth: "460px",
    lineHeight: "1.6",
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
  sendBtnDisabled: {
    opacity: 0.5,
    cursor: "not-allowed",
    background: "var(--bg-subtle)",
    color: "var(--text-hint)",
    boxShadow: "none",
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

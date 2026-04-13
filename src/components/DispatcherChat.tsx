import { useState, useRef, useEffect, useCallback, useImperativeHandle, forwardRef, memo } from "react";
import { invoke, Channel } from "@tauri-apps/api/core";
import { User, Sparkles, Wrench, ChevronRight, ChevronDown, Send } from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type {
  DispatcherMessage,
  DispatcherAgentEvent,
  DispatcherAgentTurn,
  DispatcherSettings,
  SubProcess,
} from "../types";

// ── Dispatch Approval Dialog ─────────────────────────────────────────────────

interface DispatchApprovalProps {
  dispatchId: string;
  description: string;
  permissionMode: string;
  onApprove: (dispatchId: string, description: string) => void;
  onReject: (dispatchId: string) => void;
}

function DispatchApprovalDialog({
  dispatchId,
  description,
  permissionMode,
  onApprove,
  onReject,
}: DispatchApprovalProps) {
  const [editedDescription, setEditedDescription] = useState(description);

  return (
    <div style={styles.approvalOverlay}>
      <div style={styles.approvalDialog}>
        <div style={styles.approvalHeader}>
          <span style={styles.approvalIcon}>📋</span>
          <span style={styles.approvalTitle}>Claude 子任务审查</span>
          <span style={styles.approvalBadge}>{permissionMode}</span>
        </div>
        <textarea
          style={styles.approvalTextarea}
          value={editedDescription}
          onChange={(e) => setEditedDescription(e.target.value)}
          rows={8}
        />
        <div style={styles.approvalActions}>
          <button
            style={styles.approvalRejectBtn}
            onClick={() => onReject(dispatchId)}
          >
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

// ── Tool Activity Indicator ──────────────────────────────────────────────────

const ToolIndicator = memo(function ToolIndicator({
  name,
  isRunning,
}: {
  name: string;
  isRunning: boolean;
}) {
  return (
    <div style={styles.toolIndicator}>
      <span style={styles.toolDot(isRunning)} />
      <span style={styles.toolName}>{name}</span>
      {isRunning && <span style={styles.toolSpinner}>⟳</span>}
    </div>
  );
});

// ── Message Bubble ───────────────────────────────────────────────────────────

const MessageBubble = memo(function MessageBubble({
  msg,
}: {
  msg: DispatcherMessage;
}) {
  const isUser = msg.role === "user";
  const isTool = msg.role === "tool";
  const [isToolExpanded, setIsToolExpanded] = useState(false);

  if (isTool) {
    return (
      <div style={styles.messageBubbleWrap(false)}>
        <div style={styles.messageAvatar(false)}>
          <Wrench size={13} color="var(--text-secondary)" />
        </div>
        <div
          style={{
            ...styles.messageBubble(false),
            ...styles.toolMessageBubble,
          }}
        >
          <button
            type="button"
            style={styles.toolMessageHeader}
            onClick={() => setIsToolExpanded(!isToolExpanded)}
          >
            <span style={styles.toolDot(false)} />
            <span style={styles.toolName}>{msg.toolName || "tool"} result</span>
            <div style={{ marginLeft: "auto", display: "flex", alignItems: "center" }}>
              {isToolExpanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
            </div>
          </button>
          {isToolExpanded && (
            <div
              style={{
                ...styles.selectableText,
                marginTop: "10px", 
                paddingTop: "10px",
                borderTop: "1px solid var(--border-dim)",
                fontSize: "11.5px", 
                fontFamily: "var(--font-mono)", 
                color: "var(--text-tertiary)", 
                whiteSpace: "pre-wrap", 
                maxHeight: "300px", 
                overflowY: "auto", 
                overflowX: "hidden",
                cursor: "text"
              }}
            >
              {msg.content}
            </div>
          )}
        </div>
      </div>
    );
  }

  let toolCalls: Array<{ function?: { name?: string } }> = [];
  if (msg.role === "assistant" && msg.toolCallsJson) {
    try {
      toolCalls = JSON.parse(msg.toolCallsJson);
    } catch {
      toolCalls = [];
    }
  }

  return (
    <div style={styles.messageBubbleWrap(isUser)}>
      <div style={styles.messageAvatar(isUser)}>
        {isUser ? <User size={15} color="#fff" /> : <Sparkles size={14} color="var(--accent)" />}
      </div>
      <div style={styles.messageBubble(isUser)}>
        {msg.content && (
          isUser ? (
            <div style={styles.messageText}>{msg.content}</div>
          ) : (
            <div style={styles.markdownBody} className="dispatcher-markdown">
              <ReactMarkdown remarkPlugins={[remarkGfm]}>
                {msg.content}
              </ReactMarkdown>
            </div>
          )
        )}
        
        {toolCalls.map((tc, idx) => (
          <div key={idx} style={{ marginTop: msg.content ? "8px" : "0" }}>
            <ToolIndicator name={tc.function?.name || "tool"} isRunning={false} />
          </div>
        ))}
      </div>
    </div>
  );
});

// ── DispatcherChat ───────────────────────────────────────────────────────────

interface DispatcherChatProps {
  sessionId: string;
  projectPath: string;
  subProcesses: SubProcess[];
  onDispatchApproved: (
    dispatchId: string,
    description: string,
    permissionMode: string,
    sessionId: string,
  ) => void;
  onDispatchRejected: (dispatchId: string) => void;
  onDispatchContinue: (text: string, sessionId: string) => void;
  onDispatchExit: (reason: string, sessionId: string) => void;
  onOpenSettings: () => void;
}

export interface DispatcherChatHandle {
  /** Inject dispatch result and continue the agent conversation */
  continueWithResult: (result: string, targetSessionId?: string) => void;
}

export const DispatcherChat = forwardRef<DispatcherChatHandle, DispatcherChatProps>(
  function DispatcherChat(
    {
      sessionId,
      projectPath,
      subProcesses: _subProcesses,
      onDispatchApproved,
      onDispatchRejected,
      onDispatchContinue,
      onDispatchExit,
      onOpenSettings,
    },
    ref,
  ) {
  const [messages, setMessages] = useState<DispatcherMessage[]>([]);
  const [input, setInput] = useState("");
  const [isLoading, setIsLoading] = useState(false);
  const [streamingContent, setStreamingContent] = useState("");
  const [activeTool, setActiveTool] = useState<string | null>(null);
  const [pendingDispatch, setPendingDispatch] = useState<{
    dispatchId: string;
    description: string;
    permissionMode: string;
  } | null>(null);
  const [autoApprove, setAutoApprove] = useState(false);

  const messagesEndRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const currentSessionIdRef = useRef(sessionId);
  currentSessionIdRef.current = sessionId;
  const activeRunRef = useRef(0);
  const historyLoadRef = useRef(0);
  const autoApproveRef = useRef(autoApprove);
  autoApproveRef.current = autoApprove;
  const onDispatchApprovedRef = useRef(onDispatchApproved);
  onDispatchApprovedRef.current = onDispatchApproved;
  const onDispatchContinueRef = useRef(onDispatchContinue);
  onDispatchContinueRef.current = onDispatchContinue;
  const onDispatchExitRef = useRef(onDispatchExit);
  onDispatchExitRef.current = onDispatchExit;

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
    setActiveTool(null);
    setPendingDispatch(null);

    invoke<DispatcherMessage[]>("dispatcher_list_messages", {
      workspaceId: sessionId,
    })
      .then((loaded) => {
        if (currentSessionIdRef.current !== sessionId || historyLoadRef.current !== loadId) return;
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
          setActiveTool(event.data.name);
          break;
        case "toolFinished":
          if (!isCurrentRun) return;
          setActiveTool(null);
          break;
        case "dispatchProposed": {
          const { dispatchId, description, permissionMode } = event.data;
          if (autoApproveRef.current) {
            onDispatchApprovedRef.current(dispatchId, description, permissionMode, targetSessionId);
          } else if (isCurrentRun) {
            setPendingDispatch({ dispatchId, description, permissionMode });
          }
          break;
        }
        case "dispatchContinue": {
          onDispatchContinueRef.current(event.data.text, targetSessionId);
          break;
        }
        case "dispatchExit": {
          onDispatchExitRef.current(event.data.reason, targetSessionId);
          break;
        }
        case "finished":
          if (!isCurrentRun) return;
          setMessages(event.data.messages.filter((message) => message.workspaceId === targetSessionId));
          setIsLoading(false);
          setStreamingContent("");
          setActiveTool(null);
          break;
        case "error":
          if (!isCurrentRun) return;
          setIsLoading(false);
          setStreamingContent("");
          setActiveTool(null);
          console.error("Agent error:", event.data.message);
          break;
      }
    };
    return onEvent;
  }, []);

  const handleSend = useCallback(async () => {
    const text = input.trim();
    if (!text || isLoading) return;

    setInput("");
    setIsLoading(true);
    setStreamingContent("");
    setActiveTool(null);
    setPendingDispatch(null);

    const targetSessionId = sessionId;
    const runId = ++activeRunRef.current;
    const onEvent = createEventChannel(targetSessionId, runId);

    try {
      await invoke<DispatcherAgentTurn>("dispatcher_send_message", {
        workspaceId: targetSessionId,
        projectPath,
        content: text,
        onEvent,
      });
    } catch (err) {
      console.error("dispatcher_send_message error:", err);
    } finally {
      if (currentSessionIdRef.current === targetSessionId && activeRunRef.current === runId) {
        setIsLoading(false);
      }
    }
  }, [input, isLoading, projectPath, sessionId, createEventChannel]);

  // Expose continueWithResult to parent via ref
  useImperativeHandle(ref, () => ({
    continueWithResult: async (result: string, targetSessionId = sessionId) => {
      const isCurrentSession = currentSessionIdRef.current === targetSessionId;
      const runId = isCurrentSession ? ++activeRunRef.current : activeRunRef.current;
      if (isCurrentSession) {
        setIsLoading(true);
        setStreamingContent("");
        setActiveTool(null);
        setPendingDispatch(null);
      }

      const onEvent = createEventChannel(targetSessionId, runId);

      try {
        await invoke<DispatcherAgentTurn>("dispatcher_continue_after_dispatch", {
          workspaceId: targetSessionId,
          projectPath,
          dispatchResult: result,
          onEvent,
        });
      } catch (err) {
        console.error("dispatcher_continue_after_dispatch error:", err);
      } finally {
        if (currentSessionIdRef.current === targetSessionId && activeRunRef.current === runId) {
          setIsLoading(false);
        }
      }
    },
  }), [projectPath, sessionId, createEventChannel]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        handleSend();
      }
    },
    [handleSend],
  );

  const handleApproveDispatch = useCallback(
    (dispatchId: string, description: string) => {
      const pm = pendingDispatch?.permissionMode ?? "full_access";
      setPendingDispatch(null);
      onDispatchApproved(dispatchId, description, pm, sessionId);
    },
    [pendingDispatch, onDispatchApproved, sessionId],
  );

  const handleRejectDispatch = useCallback(
    (dispatchId: string) => {
      setPendingDispatch(null);
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
      console.error("dispatcher_set_auto_approve_dispatch error:", err);
    }
  }, [autoApprove]);

  const handleClearHistory = useCallback(async () => {
    try {
      await invoke("dispatcher_clear_messages", {
        workspaceId: sessionId,
      });
      setMessages([]);
    } catch (err) {
      console.error("clear messages error:", err);
    }
  }, [sessionId]);

  const isEmpty = messages.length === 0 && !streamingContent;

  return (
    <div style={styles.container}>
      {/* Header */}
      <div style={styles.header}>
        <div style={styles.headerLeft}>
          <span style={styles.headerIcon}>🤖</span>
          <span style={styles.headerTitle}>Dispatcher Agent</span>
          {isLoading && <span style={styles.thinkingDot} />}
        </div>
        <div style={styles.headerRight}>
          <button
            style={{
              ...styles.headerBtn,
              ...(autoApprove ? styles.headerBtnActive : {}),
            }}
            onClick={handleToggleAutoApprove}
            title="开启后，调度给 Claude 子任务时不再弹出审查确认"
          >
            免确认 {autoApprove ? "开" : "关"}
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
            <div style={styles.emptyTitle}>Dispatcher Agent</div>
            <div style={styles.emptySubtitle}>
              告诉我你想做什么，我会自动规划并调度 Claude 来完成编码任务
            </div>
          </div>
        )}
        {messages.map((msg) => (
          <MessageBubble key={msg.id} msg={msg} />
        ))}
        {streamingContent && (
          <div style={styles.messageBubbleWrap(false)}>
            <div style={styles.messageAvatar(false)}>
              <Sparkles size={14} color="var(--accent)" />
            </div>
            <div style={styles.messageBubble(false)}>
              <div style={styles.markdownBody} className="dispatcher-markdown">
                <ReactMarkdown remarkPlugins={[remarkGfm]}>
                  {streamingContent}
                </ReactMarkdown>
              </div>
            </div>
          </div>
        )}
        {activeTool && <ToolIndicator name={activeTool} isRunning />}
        <div ref={messagesEndRef} />
      </div>

      {/* Input */}
      <div style={styles.inputArea}>
        <textarea
          ref={inputRef}
          style={styles.inputTextarea}
          placeholder="Send a message to Dispatcher..."
          value={input}
          onChange={(e) => setInput(e.target.value)}
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
      {pendingDispatch && (
        <DispatchApprovalDialog
          dispatchId={pendingDispatch.dispatchId}
          description={pendingDispatch.description}
          permissionMode={pendingDispatch.permissionMode}
          onApprove={handleApproveDispatch}
          onReject={handleRejectDispatch}
        />
      )}
    </div>
  );
  },
);

// ── Styles ───────────────────────────────────────────────────────────────────

const styles = {
  container: {
    display: "flex",
    flexDirection: "column" as const,
    height: "100%",
    background: "var(--bg-primary)",
  },
  header: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    padding: "8px 16px",
    borderBottom: "1px solid var(--border-primary)",
    background: "var(--bg-secondary)",
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
    fontSize: "13px",
    fontWeight: 600,
    color: "var(--text-primary)",
  },
  thinkingDot: {
    width: "8px",
    height: "8px",
    borderRadius: "50%",
    background: "var(--accent-blue)",
    animation: "pulse 1.5s ease-in-out infinite",
  },
  headerRight: {
    display: "flex",
    alignItems: "center",
    gap: "4px",
    WebkitAppRegion: "no-drag" as const,
  },
  headerBtn: {
    padding: "4px 8px",
    fontSize: "11px",
    background: "transparent",
    border: "1px solid var(--border-primary)",
    borderRadius: "4px",
    color: "var(--text-secondary)",
    cursor: "pointer",
  },
  headerBtnActive: {
    background: "var(--accent-subtle)",
    borderColor: "var(--accent)",
    color: "var(--accent)",
  },
  messageList: {
    flex: 1,
    overflowY: "auto" as const,
    padding: "12px 16px",
    display: "flex",
    flexDirection: "column" as const,
    gap: "12px",
  },
  emptyState: {
    display: "flex",
    flexDirection: "column" as const,
    alignItems: "center",
    justifyContent: "center",
    flex: 1,
    gap: "8px",
    opacity: 0.6,
  },
  emptyIcon: { fontSize: "48px" },
  emptyTitle: {
    fontSize: "18px",
    fontWeight: 600,
    color: "var(--text-primary)",
  },
  emptySubtitle: {
    fontSize: "13px",
    color: "var(--text-secondary)",
    textAlign: "center" as const,
    maxWidth: "400px",
    lineHeight: "1.5",
  },
  messageBubbleWrap: (isUser: boolean) =>
    ({
      display: "flex",
      flexDirection: isUser ? ("row-reverse" as const) : ("row" as const),
      gap: "12px",
      alignItems: "flex-start",
      marginBottom: "2px",
    }),
  messageAvatar: (isUser: boolean) => ({
    width: "30px",
    height: "30px",
    borderRadius: "10px",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    background: isUser ? "var(--accent)" : "var(--bg-card)",
    border: "1px solid var(--border-subtle)",
    boxShadow: isUser ? "0 2px 6px -1px color-mix(in srgb, var(--accent) 50%, transparent)" : "0 2px 6px -1px rgba(0,0,0,0.06)",
    flexShrink: 0,
    marginTop: "2px",
  }),
  messageBubble: (isUser: boolean) => ({
    maxWidth: "85%",
    padding: "12px 14px",
    borderRadius: isUser ? "16px 16px 4px 16px" : "16px 16px 16px 4px",
    background: isUser ? "var(--accent)" : "var(--bg-card)",
    color: isUser ? "#fff" : "var(--text-primary)",
    border: isUser ? "none" : "1px solid var(--border-dim)",
    boxShadow: isUser ? "0 2px 8px -2px color-mix(in srgb, var(--accent) 30%, transparent)" : "0 2px 8px -2px rgba(0,0,0,0.04)",
    fontSize: "13.5px",
    lineHeight: "1.6",
    wordBreak: "break-word" as const,
    userSelect: "text" as const,
    WebkitUserSelect: "text" as const,
  }),
  messageText: {
    whiteSpace: "pre-wrap" as const,
    userSelect: "text" as const,
    WebkitUserSelect: "text" as const,
  },
  markdownBody: {
    fontSize: "13.5px",
    lineHeight: "1.6",
    userSelect: "text" as const,
    WebkitUserSelect: "text" as const,
  },
  selectableText: {
    userSelect: "text" as const,
    WebkitUserSelect: "text" as const,
  },
  toolMessageBubble: {
    padding: "8px 12px",
    background: "var(--bg-subtle)",
    opacity: 0.9,
    flex: 1,
  },
  toolMessageHeader: {
    width: "100%",
    display: "flex",
    alignItems: "center",
    gap: "8px",
    padding: 0,
    fontSize: "12px",
    color: "var(--text-secondary)",
    background: "transparent",
    border: "none",
    cursor: "pointer",
    userSelect: "none" as const,
    WebkitUserSelect: "none" as const,
  },
  toolIndicator: {
    display: "flex",
    alignItems: "center",
    gap: "6px",
    padding: "5px 12px",
    fontSize: "11.5px",
    color: "var(--text-secondary)",
    background: "var(--bg-subtle)",
    borderRadius: "16px",
    border: "1px solid var(--border-dim)",
    alignSelf: "flex-start",
  },
  toolDot: (isRunning: boolean) => ({
    width: "6px",
    height: "6px",
    borderRadius: "50%",
    background: isRunning ? "var(--success, #34c759)" : "var(--text-tertiary)",
    animation: isRunning ? "pulse 1s ease-in-out infinite" : "none",
  }),
  toolName: { fontFamily: "var(--font-mono)", fontSize: "11px" },
  toolSpinner: { animation: "spin 1s linear infinite", fontSize: "12px" },
  inputArea: {
    display: "flex",
    alignItems: "flex-end",
    gap: "10px",
    padding: "12px 16px 16px",
    background: "var(--bg-primary)",
    flexShrink: 0,
  },
  inputTextarea: {
    flex: 1,
    padding: "12px 14px",
    fontSize: "13.5px",
    lineHeight: "1.5",
    background: "var(--bg-card)",
    border: "1px solid var(--border-medium)",
    borderRadius: "12px",
    color: "var(--text-primary)",
    resize: "none" as const,
    outline: "none",
    fontFamily: "inherit",
    boxShadow: "0 2px 6px rgba(0,0,0,0.02) inset",
    transition: "border-color 0.2s, box-shadow 0.2s",
  },
  sendBtn: {
    width: "38px",
    height: "38px",
    borderRadius: "12px",
    background: "var(--accent)",
    color: "#fff",
    border: "none",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    cursor: "pointer",
    transition: "opacity 0.2s, transform 0.1s",
    boxShadow: "0 2px 6px color-mix(in srgb, var(--accent) 40%, transparent)",
  },
  sendBtnDisabled: {
    opacity: 0.5,
    cursor: "not-allowed",
    background: "var(--bg-tertiary)",
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
  approvalBadge: {
    fontSize: "11px",
    padding: "3px 8px",
    borderRadius: "6px",
    background: "var(--bg-tertiary)",
    color: "var(--text-secondary)",
    fontFamily: "var(--font-mono)",
    fontWeight: 500,
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

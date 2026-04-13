import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChevronDown, ChevronRight } from "lucide-react";
import { marked } from "marked";
import { ToolActivityBubble, type ToolActivityItem } from "./ToolActivityBubble";

interface SessionContent {
  type: "text" | "tool_use" | "tool_result" | "thinking";
  text?: string;
  id?: string;
  name?: string;
  input?: string;
  result?: string;
  thinking?: string;
}

interface SessionMessage {
  role: "user" | "assistant";
  content: SessionContent[];
}

interface SessionAssistantTurn {
  id: string;
  responseParts: string[];
  thinkingParts: string[];
  tools: ToolActivityItem[];
}

type SessionDisplayItem =
  | { kind: "user"; id: string; text: string }
  | { kind: "assistant"; id: string; turn: SessionAssistantTurn };

function ThinkingBlock({ thinking }: { thinking: string }) {
  const [expanded, setExpanded] = useState(false);

  return (
    <div style={styles.thinkingWrap}>
      <button type="button" onClick={() => setExpanded((value) => !value)} style={styles.thinkingBtn}>
        {expanded ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
        <span>Thinking…</span>
      </button>
      {expanded && (
        <div className="session-selectable" style={styles.thinkingBody}>
          {thinking}
        </div>
      )}
    </div>
  );
}

function UserMessageBubble({ text }: { text: string }) {
  return (
    <div style={styles.userWrap}>
      <div style={styles.userBubble} className="session-selectable">
        {text}
      </div>
    </div>
  );
}

function AssistantTurnBubble({ turn }: { turn: SessionAssistantTurn }) {
  const responseText = turn.responseParts.join("\n\n").trim();
  if (!responseText && turn.thinkingParts.length === 0 && turn.tools.length === 0) {
    return null;
  }

  return (
    <div style={styles.assistantWrap}>
      {turn.tools.length > 0 && (
        <div style={styles.assistantSection}>
          <ToolActivityBubble tools={turn.tools} />
        </div>
      )}
      {(turn.thinkingParts.length > 0 || responseText) && (
        <div style={styles.assistantSection}>
          <div style={styles.assistantBubble}>
            {turn.thinkingParts.map((thinking, index) => (
              <ThinkingBlock key={`${turn.id}-thinking-${index}`} thinking={thinking} />
            ))}
            {responseText && (
              <div
                className="session-prose"
                dangerouslySetInnerHTML={{ __html: marked(responseText, { async: false }) as string }}
              />
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function buildSessionDisplayItems(messages: SessionMessage[]): SessionDisplayItem[] {
  const items: SessionDisplayItem[] = [];
  let currentTurn: SessionAssistantTurn | null = null;

  const ensureAssistantTurn = (seedId: string) => {
    if (currentTurn) {
      return currentTurn;
    }

    currentTurn = {
      id: `session-assistant-${seedId}`,
      responseParts: [],
      thinkingParts: [],
      tools: [],
    };
    items.push({
      kind: "assistant",
      id: currentTurn.id,
      turn: currentTurn,
    });
    return currentTurn;
  };

  messages.forEach((message, messageIndex) => {
    if (message.role === "user") {
      const text = message.content
        .filter((part) => part.type === "text")
        .map((part) => part.text ?? "")
        .join("\n")
        .trim();
      if (!text) {
        return;
      }
      currentTurn = null;
      items.push({
        kind: "user",
        id: `session-user-${messageIndex}`,
        text,
      });
      return;
    }

    const turn = ensureAssistantTurn(String(messageIndex));
    message.content.forEach((part, partIndex) => {
      if (part.type === "text" && part.text?.trim()) {
        turn.responseParts.push(part.text.trim());
        return;
      }

      if (part.type === "thinking" && part.thinking?.trim()) {
        turn.thinkingParts.push(part.thinking.trim());
        return;
      }

      if (part.type === "tool_use") {
        upsertTool(turn.tools, {
          key: part.id || `${messageIndex}-${partIndex}-${part.name || "tool"}`,
          name: part.name || "tool",
          input: part.input || "",
          status: "completed",
        });
        return;
      }

      if (part.type === "tool_result" && part.result?.trim()) {
        upsertTool(turn.tools, {
          key: part.id || `${messageIndex}-${partIndex}-${part.name || "tool"}`,
          name: part.name || "tool",
          output: part.result,
          status: "completed",
        });
      }
    });
  });

  return items.filter((item) =>
    item.kind === "user" ||
    item.turn.tools.length > 0 ||
    item.turn.thinkingParts.length > 0 ||
    item.turn.responseParts.some((part) => part.trim()),
  );
}

function upsertTool(tools: ToolActivityItem[], incoming: ToolActivityItem) {
  const index = tools.findIndex((tool) => tool.key === incoming.key);
  if (index < 0) {
    tools.push(incoming);
    return;
  }

  tools[index] = {
    ...tools[index],
    ...incoming,
    input: incoming.input ?? tools[index].input,
    output: incoming.output ?? tools[index].output,
  };
}

export function SessionView({ sessionPath }: { sessionPath: string }) {
  const [messages, setMessages] = useState<SessionMessage[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const displayItems = useMemo(() => buildSessionDisplayItems(messages), [messages]);

  useEffect(() => {
    setLoading(true);
    setError(null);
    invoke<SessionMessage[]>("read_session_messages", { sessionPath })
      .then((loadedMessages) => {
        setMessages(loadedMessages);
        setLoading(false);
      })
      .catch((err) => {
        setError(String(err));
        setLoading(false);
      });
  }, [sessionPath]);

  return (
    <div ref={scrollRef} style={styles.container}>
      {loading && <div style={styles.stateText}>Loading session…</div>}
      {error && <div style={styles.errorText}>Unable to load session: {error}</div>}
      {!loading && !error && displayItems.length === 0 && (
        <div style={styles.stateText}>No messages found in session file.</div>
      )}
      {displayItems.map((item) =>
        item.kind === "user" ? (
          <UserMessageBubble key={item.id} text={item.text} />
        ) : (
          <AssistantTurnBubble key={item.id} turn={item.turn} />
        ),
      )}
    </div>
  );
}

const styles = {
  container: {
    flex: 1,
    overflowY: "auto" as const,
    padding: "20px 28px 32px",
    display: "flex",
    flexDirection: "column" as const,
    gap: 14,
  },
  stateText: {
    color: "var(--text-hint)",
    fontSize: 13,
    padding: "12px 0",
  },
  errorText: {
    color: "var(--text-muted)",
    fontSize: 13,
    padding: "12px 0",
  },
  userWrap: {
    display: "flex",
    justifyContent: "flex-end",
  },
  userBubble: {
    maxWidth: "72%",
    padding: "11px 16px",
    background: "color-mix(in srgb, var(--accent) 12%, var(--bg-card))",
    color: "var(--text-primary)",
    borderRadius: 18,
    border: "1px solid color-mix(in srgb, var(--accent) 22%, var(--border-dim))",
    fontSize: 13.5,
    lineHeight: 1.65,
    whiteSpace: "pre-wrap" as const,
    wordBreak: "break-word" as const,
    fontFamily: "var(--font-mono)",
  },
  assistantWrap: {
    display: "flex",
    flexDirection: "column" as const,
    gap: 10,
  },
  assistantSection: {
    maxWidth: "86%",
    minWidth: 0,
  },
  assistantBubble: {
    padding: "14px 16px",
    borderRadius: 18,
    background: "color-mix(in srgb, var(--bg-card) 92%, var(--bg-subtle))",
    border: "1px solid var(--border-dim)",
    boxShadow: "var(--shadow-xs)",
    display: "flex",
    flexDirection: "column" as const,
    gap: 10,
  },
  thinkingWrap: {
    display: "flex",
    flexDirection: "column" as const,
    gap: 4,
  },
  thinkingBtn: {
    display: "flex",
    alignItems: "center",
    gap: 5,
    background: "none",
    border: "none",
    cursor: "pointer",
    padding: 0,
    color: "var(--text-hint)",
    fontSize: 11.5,
    fontStyle: "italic",
    width: "fit-content",
  },
  thinkingBody: {
    padding: "6px 12px",
    fontSize: 12,
    color: "var(--text-muted)",
    fontStyle: "italic",
    borderLeft: "2px solid var(--border-dim)",
    marginLeft: 4,
    whiteSpace: "pre-wrap" as const,
    wordBreak: "break-word" as const,
    lineHeight: 1.6,
  },
};

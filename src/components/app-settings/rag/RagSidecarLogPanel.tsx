import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  ArrowDown,
  ArrowUp,
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  Search,
  Trash2,
} from "lucide-react";
import s from "../../../styles";
import type { RagLogEntry, RagLogStream } from "../../../types";

const MAX_LOG_LINES = 2000;

function streamLabel(stream: RagLogStream): string {
  if (stream === "stderr") return "ERR";
  if (stream === "system") return "SYS";
  return "OUT";
}

function streamColor(stream: RagLogStream): string {
  if (stream === "stderr") return "var(--danger)";
  if (stream === "system") return "var(--accent)";
  return "var(--text-muted)";
}

function formatLogTime(ts: number): string {
  return new Date(ts).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

function renderHighlightedText(text: string, keyword: string) {
  const needle = keyword.trim();
  if (!needle) return text;

  const lowerText = text.toLowerCase();
  const lowerNeedle = needle.toLowerCase();
  const parts: ReactNode[] = [];
  let cursor = 0;
  let matchIndex = lowerText.indexOf(lowerNeedle);

  while (matchIndex !== -1) {
    if (matchIndex > cursor) {
      parts.push(text.slice(cursor, matchIndex));
    }
    const end = matchIndex + needle.length;
    parts.push(
      <mark key={`${matchIndex}-${end}`} style={s.ragLogHighlight}>
        {text.slice(matchIndex, end)}
      </mark>,
    );
    cursor = end;
    matchIndex = lowerText.indexOf(lowerNeedle, cursor);
  }

  if (cursor < text.length) {
    parts.push(text.slice(cursor));
  }
  return parts;
}

export function RagSidecarLogPanel() {
  const [expanded, setExpanded] = useState(false);
  const [logs, setLogs] = useState<RagLogEntry[]>([]);
  const [keyword, setKeyword] = useState("");
  const [activeMatchIndex, setActiveMatchIndex] = useState(0);
  const [followTail, setFollowTail] = useState(true);
  const [copyState, setCopyState] = useState<"idle" | "copied" | "failed">("idle");

  const scrollRef = useRef<HTMLDivElement | null>(null);
  const bottomRef = useRef<HTMLDivElement | null>(null);
  const rowRefs = useRef<Map<number, HTMLDivElement>>(new Map());

  useEffect(() => {
    let disposed = false;
    invoke<RagLogEntry[]>("rag_logs_snapshot")
      .then((snapshot) => {
        if (!disposed) {
          setLogs(snapshot.slice(-MAX_LOG_LINES));
        }
      })
      .catch(() => {});

    const unlisten = listen<RagLogEntry>("rag-log", (event) => {
      setLogs((prev) => [...prev, event.payload].slice(-MAX_LOG_LINES));
    });

    return () => {
      disposed = true;
      unlisten.then((dispose) => dispose()).catch(() => {});
    };
  }, []);

  const normalizedKeyword = keyword.trim().toLowerCase();
  const matchRowIndices = useMemo(() => {
    if (!normalizedKeyword) return [];
    return logs.flatMap((entry, index) =>
      entry.text.toLowerCase().includes(normalizedKeyword) ? [index] : [],
    );
  }, [logs, normalizedKeyword]);

  useEffect(() => {
    setActiveMatchIndex(0);
  }, [normalizedKeyword]);

  const activeLogSeq =
    matchRowIndices.length > 0
      ? logs[matchRowIndices[Math.min(activeMatchIndex, matchRowIndices.length - 1)]]?.seq
      : null;

  useEffect(() => {
    if (!expanded) return;
    if (activeLogSeq == null) return;
    rowRefs.current.get(activeLogSeq)?.scrollIntoView({
      block: "center",
      behavior: "smooth",
    });
  }, [activeLogSeq, expanded]);

  useEffect(() => {
    if (!expanded || !followTail || normalizedKeyword) return;
    bottomRef.current?.scrollIntoView({ block: "end" });
  }, [expanded, followTail, logs.length, normalizedKeyword]);

  const moveMatch = useCallback(
    (direction: 1 | -1) => {
      if (matchRowIndices.length === 0) return;
      setActiveMatchIndex((current) =>
        (current + direction + matchRowIndices.length) % matchRowIndices.length,
      );
    },
    [matchRowIndices.length],
  );

  const handleScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const distanceToBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
    setFollowTail(distanceToBottom < 24);
  }, []);

  const handleClear = useCallback(async () => {
    await invoke("rag_logs_clear");
    setLogs([]);
  }, []);

  const handleCopy = useCallback(async () => {
    const body = logs
      .map((entry) => `${formatLogTime(entry.ts)} ${streamLabel(entry.stream)} ${entry.text}`)
      .join("\n");
    try {
      await navigator.clipboard.writeText(body);
      setCopyState("copied");
    } catch {
      setCopyState("failed");
    } finally {
      window.setTimeout(() => setCopyState("idle"), 1200);
    }
  }, [logs]);

  const latestLog = logs.length > 0 ? logs[logs.length - 1] : undefined;
  const matchCountText =
    normalizedKeyword && matchRowIndices.length > 0
      ? `${Math.min(activeMatchIndex + 1, matchRowIndices.length)}/${matchRowIndices.length}`
      : normalizedKeyword
        ? "0/0"
        : `${logs.length}/${MAX_LOG_LINES}`;

  return (
    <section style={s.ragLogPanel}>
      <div style={s.ragLogHeader}>
        <button
          type="button"
          style={s.ragLogTitleButton}
          onClick={() => setExpanded((value) => !value)}
        >
          {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
          <span style={s.ragLogTitle}>Sidecar 日志</span>
          <span style={s.ragLogMeta}>{logs.length}/{MAX_LOG_LINES} 行</span>
        </button>
        {!expanded && latestLog && (
          <span style={s.ragLogCollapsedLine}>
            <span style={{ color: streamColor(latestLog.stream) }}>
              {streamLabel(latestLog.stream)}
            </span>
            {latestLog.text}
          </span>
        )}
      </div>

      {expanded && (
        <div style={s.ragLogBody}>
          <div style={s.ragLogToolbar}>
            <div style={s.ragLogSearchBox}>
              <Search size={13} />
              <input
                style={s.ragLogSearchInput}
                value={keyword}
                onChange={(event) => setKeyword(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    moveMatch(event.shiftKey ? -1 : 1);
                  }
                }}
                placeholder="搜索日志"
                spellCheck={false}
              />
              <span style={s.ragLogSearchCount}>{matchCountText}</span>
              <button
                type="button"
                style={{
                  ...s.ragLogIconButton,
                  opacity: matchRowIndices.length === 0 ? 0.45 : 1,
                  cursor: matchRowIndices.length === 0 ? "default" : "pointer",
                }}
                disabled={matchRowIndices.length === 0}
                onClick={() => moveMatch(-1)}
                title="上一个匹配"
              >
                <ArrowUp size={13} />
              </button>
              <button
                type="button"
                style={{
                  ...s.ragLogIconButton,
                  opacity: matchRowIndices.length === 0 ? 0.45 : 1,
                  cursor: matchRowIndices.length === 0 ? "default" : "pointer",
                }}
                disabled={matchRowIndices.length === 0}
                onClick={() => moveMatch(1)}
                title="下一个匹配"
              >
                <ArrowDown size={13} />
              </button>
            </div>

            <button
              type="button"
              style={s.ragLogSmallButton}
              onClick={() => {
                setFollowTail(true);
                bottomRef.current?.scrollIntoView({ block: "end", behavior: "smooth" });
              }}
              title="回到底部并跟随最新日志"
            >
              <ArrowDown size={13} />
              跟随
            </button>
            <button
              type="button"
              style={s.ragLogSmallButton}
              onClick={handleCopy}
              title="复制当前日志"
            >
              {copyState === "copied" ? <Check size={13} /> : <Copy size={13} />}
              {copyState === "copied" ? "已复制" : copyState === "failed" ? "失败" : "复制"}
            </button>
            <button
              type="button"
              style={s.ragLogSmallButton}
              onClick={handleClear}
              title="清空当前内存日志"
            >
              <Trash2 size={13} />
              清空
            </button>
          </div>

          <div ref={scrollRef} style={s.ragLogViewport} onScroll={handleScroll}>
            {logs.length === 0 ? (
              <div style={s.ragLogEmpty}>暂无日志</div>
            ) : (
              logs.map((entry) => {
                const active = entry.seq === activeLogSeq;
                return (
                  <div
                    key={entry.seq}
                    ref={(node) => {
                      if (node) {
                        rowRefs.current.set(entry.seq, node);
                      } else {
                        rowRefs.current.delete(entry.seq);
                      }
                    }}
                    style={{
                      ...s.ragLogRow,
                      background: active ? "var(--accent-subtle)" : "transparent",
                    }}
                  >
                    <span style={s.ragLogTime}>{formatLogTime(entry.ts)}</span>
                    <span style={{ ...s.ragLogStream, color: streamColor(entry.stream) }}>
                      {streamLabel(entry.stream)}
                    </span>
                    <span style={s.ragLogText}>
                      {renderHighlightedText(entry.text, keyword)}
                    </span>
                  </div>
                );
              })
            )}
            <div ref={bottomRef} />
          </div>
        </div>
      )}
    </section>
  );
}

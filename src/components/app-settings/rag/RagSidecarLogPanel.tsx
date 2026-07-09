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
import type { RagLogEntry, RagLogLevel } from "../../../types";

const MAX_LOG_LINES = 2000;

function logLevel(entry: RagLogEntry): RagLogLevel {
  if (entry.level) return entry.level;

  const text = entry.text.trimStart();
  if (text.startsWith("INFO:")) return "info";
  if (text.startsWith("DEBUG:")) return "debug";
  if (text.startsWith("WARNING:") || text.startsWith("WARN:")) return "warn";
  if (text.startsWith("ERROR:")) return "error";
  if (entry.stream === "system") return "system";
  if (entry.stream === "stderr") return "error";
  return "info";
}

function levelLabel(level: RagLogLevel): string {
  if (level === "system") return "SYS";
  if (level === "error") return "ERR";
  if (level === "warn") return "WARN";
  if (level === "debug") return "DBG";
  return "INFO";
}

function levelColor(level: RagLogLevel): string {
  if (level === "error") return "var(--danger)";
  if (level === "warn") return "var(--warning)";
  if (level === "system") return "var(--accent)";
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
      <mark key={`${matchIndex}-${end}`} className="ai-rag-log-highlight">
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
      .map((entry) => `${formatLogTime(entry.ts)} ${levelLabel(logLevel(entry))} ${entry.text}`)
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
    <section className="ai-rag-log-panel">
      <div className="ai-rag-log-header">
        <button
          type="button"
          className="ai-rag-log-title-button"
          onClick={() => setExpanded((value) => !value)}
        >
          {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
          <span className="ai-rag-log-title">Sidecar 日志</span>
          <span className="ai-rag-log-meta">{logs.length}/{MAX_LOG_LINES} 行</span>
        </button>
        {!expanded && latestLog && (
          <span className="ai-rag-log-collapsed-line">
            <span className="ai-rag-log-level" style={{ color: levelColor(logLevel(latestLog)) }}>
              {levelLabel(logLevel(latestLog))}
            </span>
            {latestLog.text}
          </span>
        )}
      </div>

      {expanded && (
        <div className="ai-rag-log-body">
          <div className="ai-rag-log-toolbar">
            <div className="ai-rag-log-search-box">
              <Search size={13} />
              <input
                className="ai-rag-log-search-input"
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
              <span className="ai-rag-log-search-count">{matchCountText}</span>
              <button
                type="button"
                className="ai-rag-log-icon-button"
                disabled={matchRowIndices.length === 0}
                onClick={() => moveMatch(-1)}
                title="上一个匹配"
              >
                <ArrowUp size={13} />
              </button>
              <button
                type="button"
                className="ai-rag-log-icon-button"
                disabled={matchRowIndices.length === 0}
                onClick={() => moveMatch(1)}
                title="下一个匹配"
              >
                <ArrowDown size={13} />
              </button>
            </div>

            <button
              type="button"
              className="ai-rag-log-small-button"
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
              className="ai-rag-log-small-button"
              onClick={handleCopy}
              title="复制当前日志"
            >
              {copyState === "copied" ? <Check size={13} /> : <Copy size={13} />}
              {copyState === "copied" ? "已复制" : copyState === "failed" ? "失败" : "复制"}
            </button>
            <button
              type="button"
              className="ai-rag-log-small-button"
              onClick={handleClear}
              title="清空当前内存日志"
            >
              <Trash2 size={13} />
              清空
            </button>
          </div>

          <div ref={scrollRef} className="ai-rag-log-viewport" onScroll={handleScroll}>
            {logs.length === 0 ? (
              <div className="ai-rag-log-empty">暂无日志</div>
            ) : (
              logs.map((entry) => {
                const active = entry.seq === activeLogSeq;
                const level = logLevel(entry);
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
                    className={active ? "ai-rag-log-row is-active" : "ai-rag-log-row"}
                  >
                    <span className="ai-rag-log-time">{formatLogTime(entry.ts)}</span>
                    <span className="ai-rag-log-level" style={{ color: levelColor(level) }}>
                      {levelLabel(level)}
                    </span>
                    <span className="ai-rag-log-text">
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

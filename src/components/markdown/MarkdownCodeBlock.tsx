import { useEffect, useMemo, useState } from "react";
import { Check, Copy, Play } from "lucide-react";
import type { PythonCodeRunRecord } from "../../types";
import { useIsDarkTheme } from "../../hooks/useIsDarkTheme";
import { highlightCodeToHtml } from "../../utils/shiki";

function escapeHtml(value: string) {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function renderPlainCodeHtml(code: string) {
  return `<pre class="markdown-code-plain"><code>${escapeHtml(code)}</code></pre>`;
}

function RunStatusBadge({ status }: { status: string }) {
  if (status === "running") {
    return <span className="python-inline-badge python-inline-badge--running">⟳ Running</span>;
  }
  if (status === "done") {
    return <span className="python-inline-badge python-inline-badge--done">✓ Done</span>;
  }
  if (status === "failed") {
    return <span className="python-inline-badge python-inline-badge--failed">✗ Failed</span>;
  }
  if (status === "stopped") {
    return <span className="python-inline-badge python-inline-badge--stopped">■ Stopped</span>;
  }
  return null;
}

function InlineRunOutput({ record }: { record: PythonCodeRunRecord }) {
  const stdout = record.stdout?.trim();
  const stderr = record.stderr?.trim();
  const isRunning = record.status === "running";

  if (!stdout && !stderr) {
    if (isRunning) {
      return (
        <div className="python-inline-output">
          <div className="python-inline-running">
            <span className="python-inline-spinner" />
            <span>Running…</span>
          </div>
        </div>
      );
    }
    return null;
  }

  return (
    <div className="python-inline-output">
      {stdout && (
        <pre className="python-inline-stdout"><code>{stdout}</code></pre>
      )}
      {stderr && (
        <pre className="python-inline-stderr"><code>{stderr}</code></pre>
      )}
    </div>
  );
}

export function MarkdownCodeBlock({
  code,
  language,
  messageId,
  codeBlockIndex,
  codeHash,
  onRunPython,
  runRecord,
  compact = false,
  streaming = false,
}: {
  code: string;
  language?: string | null;
  messageId?: string;
  codeBlockIndex?: number;
  codeHash?: string;
  onRunPython?: (target: { messageId: string; codeBlockIndex: number; code: string; codeHash: string }) => void;
  runRecord?: PythonCodeRunRecord | null;
  compact?: boolean;
  streaming?: boolean;
}) {
  const isDark = useIsDarkTheme();
  const fallbackHtml = useMemo(() => renderPlainCodeHtml(code), [code]);
  const [highlighted, setHighlighted] = useState<{ code: string; html: string } | null>(null);
  const [copied, setCopied] = useState(false);
  const resolvedLanguage = useMemo(
    () => (language?.trim() ? language.trim().toLowerCase() : "text"),
    [language],
  );
  const canRunPython =
    !streaming &&
    Boolean(messageId) &&
    typeof codeBlockIndex === "number" &&
    Boolean(codeHash) &&
    (resolvedLanguage === "python" || resolvedLanguage === "py");
  // During streaming, always use plain fallback to avoid highlight↔fallback flash
  const renderedHtml = streaming
    ? fallbackHtml
    : (highlighted?.code === code ? highlighted.html : fallbackHtml);

  useEffect(() => {
    if (streaming) return;
    let cancelled = false;
    setHighlighted({ code, html: fallbackHtml });

    highlightCodeToHtml(code, resolvedLanguage, isDark)
      .then((html) => {
        if (!cancelled) {
          setHighlighted({ code, html: html || fallbackHtml });
        }
      })
      .catch(() => {
        if (!cancelled) {
          setHighlighted({ code, html: fallbackHtml });
        }
      });

    return () => {
      cancelled = true;
    };
  }, [code, fallbackHtml, isDark, resolvedLanguage, streaming]);

  useEffect(() => {
    if (!copied) {
      return;
    }

    const timer = window.setTimeout(() => setCopied(false), 1800);
    return () => window.clearTimeout(timer);
  }, [copied]);

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  }

  const showRunButton = canRunPython && !runRecord;
  const isRunning = runRecord?.status === "running";

  return (
    <div className={`markdown-code-block${compact ? " markdown-code-block--compact" : ""}${runRecord ? " markdown-code-block--has-run" : ""}`}>
      <div className="markdown-code-toolbar">
        <span className="markdown-code-language">{resolvedLanguage}</span>
        <div className="markdown-code-actions">
          {runRecord && <RunStatusBadge status={runRecord.status} />}
          {showRunButton && (
            <button
              type="button"
              className="markdown-code-copy"
              onClick={() =>
                onRunPython?.({
                  messageId: messageId!,
                  codeBlockIndex: codeBlockIndex!,
                  code,
                  codeHash: codeHash!,
                })
              }
              title="运行 Python 代码"
            >
              <Play size={13} />
              Run
            </button>
          )}
          {isRunning && (
            <button
              type="button"
              className="markdown-code-copy"
              disabled
              title="正在执行…"
            >
              <Play size={13} />
              Running…
            </button>
          )}
          <button type="button" className="markdown-code-copy" onClick={handleCopy}>
            {copied ? <Check size={13} /> : <Copy size={13} />}
            {copied ? "Copied" : "Copy"}
          </button>
        </div>
      </div>
      <div className="markdown-code-content" dangerouslySetInnerHTML={{ __html: renderedHtml }} />
      {runRecord && <InlineRunOutput record={runRecord} />}
    </div>
  );
}

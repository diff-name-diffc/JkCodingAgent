import { useEffect, useMemo, useState } from "react";
import { Check, Copy } from "lucide-react";
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

export function MarkdownCodeBlock({
  code,
  language,
  compact = false,
}: {
  code: string;
  language?: string | null;
  compact?: boolean;
}) {
  const isDark = useIsDarkTheme();
  const fallbackHtml = useMemo(() => renderPlainCodeHtml(code), [code]);
  const [highlighted, setHighlighted] = useState<{ code: string; html: string } | null>(null);
  const [copied, setCopied] = useState(false);
  const resolvedLanguage = useMemo(
    () => (language?.trim() ? language.trim().toLowerCase() : "text"),
    [language],
  );
  const renderedHtml = highlighted?.code === code ? highlighted.html : fallbackHtml;

  useEffect(() => {
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
  }, [code, fallbackHtml, isDark, resolvedLanguage]);

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

  return (
    <div className={`markdown-code-block${compact ? " markdown-code-block--compact" : ""}`}>
      <div className="markdown-code-toolbar">
        <span className="markdown-code-language">{resolvedLanguage}</span>
        <button type="button" className="markdown-code-copy" onClick={handleCopy}>
          {copied ? <Check size={13} /> : <Copy size={13} />}
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
      <div className="markdown-code-content" dangerouslySetInnerHTML={{ __html: renderedHtml }} />
    </div>
  );
}

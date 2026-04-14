import { useEffect, useMemo, useState } from "react";
import { Check, Copy } from "lucide-react";
import { useIsDarkTheme } from "../../hooks/useIsDarkTheme";
import { highlightCodeToHtml } from "../../utils/shiki";

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
  const [highlighted, setHighlighted] = useState<string>("");
  const [copied, setCopied] = useState(false);
  const resolvedLanguage = useMemo(
    () => (language?.trim() ? language.trim().toLowerCase() : "text"),
    [language],
  );

  useEffect(() => {
    let cancelled = false;

    highlightCodeToHtml(code, resolvedLanguage, isDark)
      .then((html) => {
        if (!cancelled) {
          setHighlighted(html);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setHighlighted("");
        }
      });

    return () => {
      cancelled = true;
    };
  }, [code, isDark, resolvedLanguage]);

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
      <div
        className="markdown-code-content"
        dangerouslySetInnerHTML={{ __html: highlighted }}
      />
    </div>
  );
}

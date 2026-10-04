import { useEffect, useMemo, useState } from "react";
import { Check, Copy } from "lucide-react";
import { highlightCodeToHtml } from "../../utils/shiki";
import { useIsDarkTheme } from "../../hooks/useIsDarkTheme";

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

/**
 * 遗留 react-markdown 管线的代码块（Shiki 高亮 + 复制）。
 * 聊天 streamdown 管线的代码渲染在 chat/shiki-code-plugin.ts，互不复用。
 */
export function MarkdownCodeBlock({
  code,
  language,
  compact = false,
}: {
  code: string;
  language?: string | null;
  compact?: boolean;
}) {
  const fallbackHtml = useMemo(() => renderPlainCodeHtml(code), [code]);
  const [highlighted, setHighlighted] = useState<{
    code: string;
    isDark: boolean;
    html: string;
  } | null>(null);
  const [copied, setCopied] = useState(false);
  const isDark = useIsDarkTheme();
  const resolvedLanguage = useMemo(
    () => (language?.trim() ? language.trim().toLowerCase() : "text"),
    [language],
  );
  // 主题也是缓存有效性的一部分：isDark 变化后、新高亮结果返回前，
  // 旧主题的 HTML 不能继续当作有效缓存渲染（否则主题切换瞬间闪烁旧配色）。
  const renderedHtml =
    highlighted?.code === code && highlighted.isDark === isDark ? highlighted.html : fallbackHtml;

  useEffect(() => {
    let cancelled = false;

    highlightCodeToHtml(code, resolvedLanguage, isDark)
      .then((html) => {
        if (!cancelled) {
          setHighlighted({ code, isDark, html: html || fallbackHtml });
        }
      })
      .catch(() => {
        if (!cancelled) {
          setHighlighted({ code, isDark, html: fallbackHtml });
        }
      });

    return () => {
      cancelled = true;
    };
  }, [code, fallbackHtml, resolvedLanguage, isDark]);

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
        <div className="markdown-code-actions">
          <button type="button" className="markdown-code-copy" onClick={handleCopy}>
            {copied ? <Check size={13} /> : <Copy size={13} />}
            {copied ? "已复制" : "复制"}
          </button>
        </div>
      </div>
      <div className="markdown-code-content" dangerouslySetInnerHTML={{ __html: renderedHtml }} />
    </div>
  );
}

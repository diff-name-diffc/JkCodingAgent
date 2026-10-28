import { useEffect, useMemo, useState } from "react";
import { Check, Copy } from "lucide-react";
import { highlightCodeToHtml } from "../../utils/shiki";
import { useIsDarkTheme } from "../../hooks/useIsDarkTheme";
import { copyTextToClipboard } from "../../lib/clipboard-fallback";
import { toast } from "../Toast";

function escapeHtml(value: string) {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function renderPlainCodeHtml(code: string) {
  return `<pre><code>${escapeHtml(code)}</code></pre>`;
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
      // 统一走 copyTextToClipboard（原生插件优先）：裸 navigator.clipboard
      // 在 WKWebView 会静默失败，catch 里又不兜底，复制就会无反馈落空。
      await copyTextToClipboard(code);
      setCopied(true);
    } catch (error) {
      // 复制失败时清掉上一次成功留下的 ✓，避免按钮停留在「已复制」误导。
      setCopied(false);
      toast.warning(`复制代码失败：${error instanceof Error ? error.message : String(error)}`);
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

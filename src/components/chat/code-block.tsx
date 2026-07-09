import * as React from "react";
import { Check, Copy } from "lucide-react";
import { cn } from "../../lib/cn";
import { Button } from "../ui/button";
import { useIsDarkTheme } from "../../hooks/useIsDarkTheme";
import { highlightCodeToHtml } from "../../utils/shiki";

/**
 * Standalone, self-contained code block for the refactored Chat UI.
 *
 * Reuses the existing shiki highlighter (utils/shiki.ts) — same lazy language
 * loading and github-dark/light themes as the rest of the app — so light/dark
 * mode and language support stay perfectly in sync with MarkdownCodeBlock.
 *
 * Differences from the legacy MarkdownCodeBlock:
 *   - Tailwind + shadcn styling instead of inline styles
 *   - Presentation-only. Python execution is wired through the shared
 *     MarkdownRenderer path so historical run records stay in one place.
 *   - Highlights asynchronously, showing the raw code first so streaming
 *     output never blocks
 */
export interface CodeBlockProps {
  code: string;
  language?: string | null;
  className?: string;
  /** When true, skips re-highlighting on every render (streaming tail). */
  streaming?: boolean;
}

export function CodeBlock({
  code,
  language,
  className,
  streaming,
}: CodeBlockProps) {
  const isDark = useIsDarkTheme();
  const [html, setHtml] = React.useState<string | null>(null);

  React.useEffect(() => {
    let cancelled = false;
    // Highlight async so a fast-streaming tail never blocks the main thread.
    highlightCodeToHtml(code, language, isDark)
      .then((rendered) => {
        if (!cancelled) setHtml(rendered);
      })
      .catch(() => {
        if (!cancelled) setHtml(null);
      });
    return () => {
      cancelled = true;
    };
  }, [code, language, isDark, streaming]);

  return (
    <CodeBlockShell
      code={code}
      language={language}
      className={className}
      html={html}
    />
  );
}

const CodeBlockShell = React.memo(function CodeBlockShell({
  code,
  language,
  className,
  html,
}: {
  code: string;
  language?: string | null;
  className?: string;
  html: string | null;
}) {
  const [copied, setCopied] = React.useState(false);

  const onCopy = React.useCallback(async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    } catch {
      // Clipboard may be unavailable in some Tauri contexts; fail silently.
    }
  }, [code]);

  const langLabel = (language || "text").toLowerCase();

  return (
    <div
      className={cn(
        "group relative my-3 overflow-hidden rounded-lg border border-border bg-muted/40",
        className,
      )}
    >
      <div className="flex items-center justify-between border-b border-border/70 bg-secondary/60 px-3 py-1.5">
        <span className="font-mono text-[11px] uppercase tracking-wide text-muted-foreground">
          {langLabel}
        </span>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="复制代码"
          onClick={onCopy}
          className="opacity-0 transition-opacity group-hover:opacity-100"
        >
          {copied ? (
            <Check className="h-3.5 w-3.5 text-success" />
          ) : (
            <Copy className="h-3.5 w-3.5" />
          )}
        </Button>
      </div>
      <div className="chat-scroll overflow-x-auto">
        {html ? (
          <div
            className="shiki-surface text-[13px] leading-relaxed"
            // Shiki returns sanitized, scoped HTML. Safe to inject here.
            dangerouslySetInnerHTML={{ __html: html }}
          />
        ) : (
          <pre className="px-4 py-3 font-mono text-[13px] leading-relaxed text-foreground">
            <code>{code}</code>
          </pre>
        )}
      </div>
    </div>
  );
});

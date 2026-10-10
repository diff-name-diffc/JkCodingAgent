import * as React from "react";
import { highlightCodeToHtml } from "../../utils/shiki";
import { shikiCacheKey, shikiHighlightCache } from "../../utils/shiki-cache";
import { useIsDarkTheme } from "../../hooks/useIsDarkTheme";
import { usePersistedToggle } from "./row-ui-state";

const MAX_COLLAPSED_OUTPUT_LINES = 20;

export function ToolCallData({
  label,
  value,
  persistKey,
}: {
  label: "输入" | "输出" | "输出 · 回传模型";
  value: unknown;
  persistKey: string;
}) {
  const [showAll, setShowAll] = usePersistedToggle(persistKey, false);
  const content = React.useMemo(() => serializeData(value), [value]);
  const lines = React.useMemo(() => content.split("\n"), [content]);
  const isLong = lines.length > MAX_COLLAPSED_OUTPUT_LINES;
  const visibleContent =
    isLong && !showAll ? lines.slice(0, MAX_COLLAPSED_OUTPUT_LINES).join("\n") : content;

  return (
    <section>
      <div className="mb-1.5 text-[11px] font-medium text-muted-foreground">{label}</div>
      <JsonCode value={visibleContent} />
      {isLong && (
        <button
          type="button"
          onClick={() => setShowAll((current) => !current)}
          className="mt-1.5 text-[11px] font-medium text-primary hover:text-primary-hover"
          aria-expanded={showAll}
        >
          {showAll ? "收起" : `展开全部（${lines.length} 行）`}
        </button>
      )}
    </section>
  );
}

function JsonCode({ value }: { value: string }) {
  const isDark = useIsDarkTheme();
  const [highlighted, setHighlighted] = React.useState<string | null>(
    () => shikiHighlightCache.get(shikiCacheKey(value, "json", isDark)) ?? null,
  );

  React.useEffect(() => {
    let active = true;
    const cached = shikiHighlightCache.get(shikiCacheKey(value, "json", isDark));
    setHighlighted(cached ?? null);
    if (cached !== undefined) return;
    highlightCodeToHtml(value, "json", isDark)
      .then((html) => {
        if (active) setHighlighted(html);
      })
      .catch(() => {
        if (active) setHighlighted(null);
      });
    return () => {
      active = false;
    };
  }, [value, isDark]);

  if (!highlighted) {
    return (
      <pre className="chat-scroll max-h-80 overflow-auto rounded-md bg-muted/60 p-2.5 font-mono text-[11px] leading-[1.55] text-foreground">
        {value}
      </pre>
    );
  }

  return (
    <div
      className="ai-tool-call-code chat-scroll max-h-80 overflow-auto rounded-md bg-muted/60 font-mono text-[11px] leading-[1.55]"
      dangerouslySetInnerHTML={{ __html: highlighted }}
    />
  );
}

/** 展示前序列化：JSON 字符串美化重排，非 JSON 原文保留，对象直接序列化。 */
export function serializeData(value: unknown): string {
  if (typeof value === "string") {
    try {
      return JSON.stringify(JSON.parse(value), null, 2);
    } catch {
      return value;
    }
  }
  return JSON.stringify(value, null, 2) ?? String(value);
}

import { createContext, memo, useContext, useEffect, useMemo, useRef, useState } from "react";
import type { Components } from "react-markdown";
import ReactMarkdown, { defaultUrlTransform } from "react-markdown";
import rehypeKatex from "rehype-katex";
import rehypeRaw from "rehype-raw";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import "katex/dist/katex.min.css";
import { MarkdownCodeBlock } from "./MarkdownCodeBlock";
import { MarkdownImage } from "./MarkdownImage";

const StreamingContext = createContext(false);

const safeSchema = {
  ...defaultSchema,
  tagNames: [
    ...(defaultSchema.tagNames || []),
    "video",
    "audio",
    "source",
    "details",
    "summary",
  ],
  attributes: {
    ...defaultSchema.attributes,
    "*": [...(defaultSchema.attributes?.["*"] || []), "className", "style"],
    video: ["src", "controls", "width", "height", "muted", "autoplay", "loop"],
    audio: ["src", "controls"],
    source: ["src", "type"],
    details: ["open"],
    img: [
      "src",
      "alt",
      "width",
      "height",
      "loading",
      ...(defaultSchema.attributes?.img || []),
    ],
    a: ["href", "target", "rel", ...(defaultSchema.attributes?.a || [])],
    code: ["className"],
    span: ["className", "style", ...(defaultSchema.attributes?.span || [])],
    div: ["className", "style"],
    td: ["align", "className"],
    th: ["align", "className"],
  },
};

function customUrlTransform(url: string) {
  if (
    url.startsWith("data:image/") ||
    url.startsWith("asset://") ||
    url.startsWith("http://asset.localhost/") ||
    url.startsWith("/")
  ) {
    return url;
  }
  return defaultUrlTransform(url);
}

function normalizeSingleLineMathBlocks(content: string) {
  let fenceMarker: string | null = null;

  return content
    .split("\n")
    .map((line) => {
      const fenceMatch = line.match(/^ {0,3}(`{3,}|~{3,})/);
      if (fenceMatch) {
        const marker = fenceMatch[1];
        if (!fenceMarker) {
          fenceMarker = marker;
        } else if (marker[0] === fenceMarker[0] && marker.length >= fenceMarker.length) {
          fenceMarker = null;
        }
        return line;
      }

      if (fenceMarker) {
        return line;
      }

      const leadingWhitespaceLength = line.length - line.trimStart().length;
      const leadingWhitespace = line.slice(0, leadingWhitespaceLength);
      const trimmed = line.trim();

      if (trimmed.startsWith("$$") && trimmed.endsWith("$$") && trimmed.length > 4) {
        return `${leadingWhitespace}$$\n${leadingWhitespace}${trimmed.slice(2, -2).trim()}\n${leadingWhitespace}$$`;
      }

      return line;
    })
    .join("\n");
}

const markdownComponents: Components = {
  code({ className, children }) {
    const rawCode = String(children).replace(/\n$/, "");
    const language = className?.match(/language-([\w-]+)/)?.[1];
    const isBlock = Boolean(className) || rawCode.includes("\n");

    if (!isBlock) {
      return <code className="markdown-inline-code">{rawCode}</code>;
    }

    return <StreamingCodeBlock code={rawCode} language={language} />;
  },
  pre({ children }) {
    return <>{children}</>;
  },
  table({ children }) {
    return (
      <div className="markdown-table-wrap">
        <table>{children}</table>
      </div>
    );
  },
  img({ src, alt }) {
    return <MarkdownImage src={src} alt={alt} />;
  },
};

/** Reads streaming flag from context and passes to MarkdownCodeBlock */
function StreamingCodeBlock({ code, language }: { code: string; language?: string | null }) {
  const streaming = useContext(StreamingContext);
  return <MarkdownCodeBlock code={code} language={language} compact streaming={streaming} />;
}

const LARGE_TEXT_THRESHOLD = 10_000;

export const MarkdownRenderer = memo(function MarkdownRenderer({
  content,
  variant = "chat",
  streaming = false,
}: {
  content: string;
  variant?: "chat" | "document";
  streaming?: boolean;
}) {
  // When streaming, throttle markdown parse to ~7fps to avoid 50/s full AST re-parses
  const [throttledContent, setThrottledContent] = useState(content);
  const latestContentRef = useRef(content);

  useEffect(() => {
    latestContentRef.current = content;
    if (!streaming) {
      setThrottledContent(content);
      return;
    }
    const id = setTimeout(() => setThrottledContent(latestContentRef.current), 150);
    return () => clearTimeout(id);
  }, [content, streaming]);

  const effectiveContent = streaming ? throttledContent : content;
  const normalizedContent = useMemo(
    () => normalizeSingleLineMathBlocks(effectiveContent),
    [effectiveContent],
  );

  // Defer full markdown rendering for large texts to avoid blocking the main thread
  const isLarge = normalizedContent.length > LARGE_TEXT_THRESHOLD;
  const [readyForRender, setReadyForRender] = useState(false);

  useEffect(() => {
    if (!isLarge) {
      setReadyForRender(true);
      return;
    }
    setReadyForRender(false);
    const id = requestAnimationFrame(() => setReadyForRender(true));
    return () => cancelAnimationFrame(id);
  }, [isLarge, normalizedContent]);

  if (isLarge && !readyForRender) {
    return (
      <div className={`markdown-surface markdown-surface--${variant}`}>
        <pre style={{ whiteSpace: "pre-wrap", wordBreak: "break-word" }}>
          {normalizedContent}
        </pre>
      </div>
    );
  }

  return (
    <div className={`markdown-surface markdown-surface--${variant}`}>
      <StreamingContext.Provider value={streaming}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkMath]}
        rehypePlugins={[rehypeRaw, [rehypeSanitize, safeSchema], rehypeKatex]}
        components={markdownComponents}
        urlTransform={customUrlTransform}
      >
        {normalizedContent}
      </ReactMarkdown>
      </StreamingContext.Provider>
    </div>
  );
});

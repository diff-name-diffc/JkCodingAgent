import { memo, useDeferredValue, useMemo } from "react";
import type { Components } from "react-markdown";
import ReactMarkdown, { defaultUrlTransform } from "react-markdown";
import rehypeKatex from "rehype-katex";
import rehypeRaw from "rehype-raw";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import "katex/dist/katex.min.css";
import { MarkdownCodeBlock } from "./MarkdownCodeBlock";
import { MarkdownImage } from "./MarkdownImage";

function customUrlTransform(url: string) {
  if (url.startsWith("data:image/") || url.startsWith("asset://") || url.startsWith("http://asset.localhost/")) {
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
        return `${leadingWhitespace}$$\n${trimmed.slice(2, -2).trim()}\n${leadingWhitespace}$$`;
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

    return <MarkdownCodeBlock code={rawCode} language={language} compact />;
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

export const MarkdownRenderer = memo(function MarkdownRenderer({
  content,
  variant = "chat",
}: {
  content: string;
  variant?: "chat" | "document";
}) {
  const deferredContent = useDeferredValue(content);
  const normalizedContent = useMemo(
    () => normalizeSingleLineMathBlocks(deferredContent),
    [deferredContent],
  );

  return (
    <div className={`markdown-surface markdown-surface--${variant}`}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkMath]}
        rehypePlugins={[rehypeRaw, rehypeKatex]}
        components={markdownComponents}
        urlTransform={customUrlTransform}
      >
        {normalizedContent}
      </ReactMarkdown>
    </div>
  );
});

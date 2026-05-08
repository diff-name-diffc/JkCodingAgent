import { memo, useDeferredValue } from "react";
import type { Components } from "react-markdown";
import ReactMarkdown, { defaultUrlTransform } from "react-markdown";
import rehypeRaw from "rehype-raw";
import remarkGfm from "remark-gfm";
import { MarkdownCodeBlock } from "./MarkdownCodeBlock";
import { MarkdownImage } from "./MarkdownImage";

function customUrlTransform(url: string) {
  if (url.startsWith("data:image/")) {
    return url;
  }
  return defaultUrlTransform(url);
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

  return (
    <div className={`markdown-surface markdown-surface--${variant}`}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={[rehypeRaw]}
        components={markdownComponents}
        urlTransform={customUrlTransform}
      >
        {deferredContent}
      </ReactMarkdown>
    </div>
  );
});

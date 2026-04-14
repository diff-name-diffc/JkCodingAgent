import { memo, useDeferredValue } from "react";
import type { Components } from "react-markdown";
import ReactMarkdown from "react-markdown";
import rehypeRaw from "rehype-raw";
import remarkGfm from "remark-gfm";
import { MarkdownCodeBlock } from "./MarkdownCodeBlock";

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
      >
        {deferredContent}
      </ReactMarkdown>
    </div>
  );
});

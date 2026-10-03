import { memo, useMemo } from "react";
import type { Components } from "react-markdown";
import ReactMarkdown from "react-markdown";
import rehypeKatex from "rehype-katex";
import rehypeRaw from "rehype-raw";
import rehypeSanitize from "rehype-sanitize";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import "katex/dist/katex.min.css";
import { normalizeMarkdownMath } from "../../lib/normalize-math";
import { MarkdownCodeBlock } from "./MarkdownCodeBlock";
import { MarkdownImage } from "./MarkdownImage";
import { MarkdownLink } from "./MarkdownLink";
import { chatSafeSchema, chatUrlTransform } from "./sanitize-schema";
import { useDeferredContent } from "./use-deferred-content";

/**
 * 遗留 react-markdown 管线的实现（文件查看器 / 子智能体结果 / Python 运行
 * 记录 / 工作流节点输出）。经 ./MarkdownRenderer.tsx 的 lazy 外壳按需加载，
 * 不要新增对本文件的静态 import。
 *
 * 这些表面只渲染终态 markdown，不接 Python 运行按钮与流式参数——那是
 * 聊天 streamdown 管线（components/chat/markdown-renderer.tsx）的能力。
 */

export interface MarkdownRendererProps {
  content: string;
  variant?: "chat" | "document";
}

export const MarkdownRenderer = memo(function MarkdownRenderer({
  content,
  variant = "chat",
}: MarkdownRendererProps) {
  // 大文本首帧降级（与聊天 streamdown 管线共用同一 hook）。
  const { effectiveContent, deferred } = useDeferredContent(content, false);
  const normalizedContent = useMemo(
    () => (deferred ? effectiveContent : normalizeMarkdownMath(effectiveContent)),
    [effectiveContent, deferred],
  );

  if (deferred) {
    return (
      <div className={`markdown-surface markdown-surface--${variant}`}>
        <pre style={{ whiteSpace: "pre-wrap", wordBreak: "break-word" }}>
          {effectiveContent}
        </pre>
      </div>
    );
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
    a({ href, title, children }) {
      return (
        <MarkdownLink href={href} title={title}>
          {children}
        </MarkdownLink>
      );
    },
  };

  return (
    <div className={`markdown-surface markdown-surface--${variant}`}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkMath]}
        rehypePlugins={[rehypeRaw, [rehypeSanitize, chatSafeSchema], rehypeKatex]}
        components={markdownComponents}
        urlTransform={chatUrlTransform}
      >
        {normalizedContent}
      </ReactMarkdown>
    </div>
  );
});

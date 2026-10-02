import { createContext, memo, useContext, useMemo } from "react";
import type { Components } from "react-markdown";
import ReactMarkdown from "react-markdown";
import rehypeKatex from "rehype-katex";
import rehypeRaw from "rehype-raw";
import rehypeSanitize from "rehype-sanitize";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import "katex/dist/katex.min.css";
import type { PythonCodeRunRecord } from "../../types";
import { stableHash } from "../../lib/stable-hash";
import { normalizeMarkdownMath } from "../../lib/normalize-math";
import { MarkdownCodeBlock } from "./MarkdownCodeBlock";
import { MarkdownImage } from "./MarkdownImage";
import { MarkdownLink } from "./MarkdownLink";
import { chatSafeSchema, chatUrlTransform } from "./sanitize-schema";
import { useDeferredContent } from "./use-deferred-content";

/**
 * 遗留 react-markdown 管线的实现（文件查看器 / 子智能体结果 / Python 运行
 * 记录）。经 ./MarkdownRenderer.tsx 的 lazy 外壳按需加载，不要新增对
 * 本文件的静态 import。
 */

export interface MarkdownRendererProps {
  content: string;
  variant?: "chat" | "document";
  streaming?: boolean;
  messageId?: string;
  onRunPython?: (target: {
    messageId: string;
    codeBlockIndex: number;
    code: string;
    codeHash: string;
  }) => void;
  pythonRunRecords?: Record<string, PythonCodeRunRecord>;
}

const StreamingContext = createContext(false);

/** Reads streaming flag from context and passes to MarkdownCodeBlock */
function StreamingCodeBlock({
  code,
  language,
  messageId,
  codeBlockIndex,
  codeHash,
  onRunPython,
  runRecord,
}: {
  code: string;
  language?: string | null;
  messageId?: string;
  codeBlockIndex?: number;
  codeHash: string;
  onRunPython?: (target: { messageId: string; codeBlockIndex: number; code: string; codeHash: string }) => void;
  runRecord?: PythonCodeRunRecord | null;
}) {
  const streaming = useContext(StreamingContext);
  return (
    <MarkdownCodeBlock
      code={code}
      language={language}
      messageId={messageId}
      codeBlockIndex={codeBlockIndex}
      codeHash={codeHash}
      onRunPython={onRunPython}
      runRecord={runRecord}
      compact
      streaming={streaming}
    />
  );
}

export const MarkdownRenderer = memo(function MarkdownRenderer({
  content,
  variant = "chat",
  streaming = false,
  messageId,
  onRunPython,
  pythonRunRecords,
}: MarkdownRendererProps) {
  // 流式节流 + 大文本首帧降级（与聊天 streamdown 管线共用同一 hook）。
  const { effectiveContent, deferred } = useDeferredContent(content, streaming);
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

  let codeBlockIndex = 0;
  const markdownComponents: Components = {
    code({ className, children }) {
      const rawCode = String(children).replace(/\n$/, "");
      const language = className?.match(/language-([\w-]+)/)?.[1];
      const isBlock = Boolean(className) || rawCode.includes("\n");

      if (!isBlock) {
        return <code className="markdown-inline-code">{rawCode}</code>;
      }

      const currentIndex = codeBlockIndex;
      codeBlockIndex += 1;
      const hash = stableHash(rawCode);
      const record = messageId && pythonRunRecords
        ? pythonRunRecords[`${messageId}:${hash}`] ?? null
        : null;
      return (
        <StreamingCodeBlock
          code={rawCode}
          language={language}
          messageId={messageId}
          codeBlockIndex={currentIndex}
          codeHash={hash}
          onRunPython={onRunPython}
          runRecord={record}
        />
      );
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
      <StreamingContext.Provider value={streaming}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkMath]}
        rehypePlugins={[rehypeRaw, [rehypeSanitize, chatSafeSchema], rehypeKatex]}
        components={markdownComponents}
        urlTransform={chatUrlTransform}
      >
        {normalizedContent}
      </ReactMarkdown>
      </StreamingContext.Provider>
    </div>
  );
});

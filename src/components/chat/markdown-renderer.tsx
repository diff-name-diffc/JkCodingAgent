import { createContext, memo, useContext, useMemo } from "react";
import type { MouseEvent } from "react";
import { Play } from "lucide-react";
import {
  CodeBlock,
  CodeBlockCopyButton,
  Streamdown,
  type Components,
  type CustomRendererProps,
} from "streamdown";
import { createCodePlugin } from "@streamdown/code";
import { createMathPlugin } from "@streamdown/math";
import { mermaid } from "@streamdown/mermaid";
import rehypeRaw from "rehype-raw";
import rehypeSanitize from "rehype-sanitize";
import "streamdown/styles.css";
import "katex/dist/katex.min.css";
import type { PythonCodeRunRecord } from "../../types";
import { cn } from "../../lib/cn";
import { TEAL_DARK_THEME, TEAL_LIGHT_THEME } from "../../utils/shiki";
import { normalizeLatexMathDelimiters, normalizeMathCodeFences } from "../../lib/normalize-math";
import { MarkdownImage } from "../markdown/MarkdownImage";
import { chatSafeSchema } from "../markdown/sanitize-schema";
import { useMarkdownLinkHandler } from "../markdown/MarkdownLinkContext";
import { useKatexCopy } from "./katex-copy";

/**
 * Streamdown-based markdown renderer for the chat surface (AI text messages).
 *
 * Replaces the legacy react-markdown pipeline for chat bubbles with
 * <Streamdown>, which is purpose-built for token-streamed markdown:
 *   - unterminated block parsing (remend) while streaming
 *   - per-block memoization — during streaming only the tail block re-renders
 *   - built-in blinking caret on the last block (mode="streaming")
 *   - GFM tables, KaTeX math, Mermaid diagrams, Shiki highlighting via plugins
 *
 * Styling hooks live in styles/tailwind.css under `.ai-streamdown` (code card,
 * table frame). User messages do NOT go through this — they stay plain text
 * (see user-message.tsx).
 *
 * The Python "Run" button / inline run output is preserved from the legacy
 * renderer via a streamdown custom renderer for python code blocks, wired
 * through PythonRunContext (messageId + codeBlockIndex + codeHash stay
 * compatible with existing python_runs DB records).
 */

export interface MarkdownRendererProps {
  content: string;
  streaming?: boolean;
  messageId?: string;
  onRunPython?: (target: {
    messageId: string;
    codeBlockIndex: number;
    code: string;
    codeHash: string;
  }) => void;
  pythonRunRecords?: Record<string, PythonCodeRunRecord>;
  className?: string;
}

/** Fast, non-crypto hash for stable code block identification. */
function stableHash(text: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = (h * 0x01000193) >>> 0;
  }
  return h.toString(36);
}

function isBrowserUrl(url: string | undefined): url is string {
  if (!url) return false;
  try {
    const parsed = new URL(url);
    return parsed.protocol === "http:" || parsed.protocol === "https:";
  } catch {
    return false;
  }
}

/**
 * remark-math does not treat a single-line `$$…$$` as a math block; expand
 * those to the multi-line form (same normalization the legacy renderer used).
 */
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

/**
 * Positional index of every fenced code block in the message, keyed by code
 * hash (first occurrence wins — identical blocks share a run record). The
 * index must stay compatible with the python_runs table PK
 * (workspace_id, message_id, code_block_index).
 */
function indexCodeBlocks(content: string): Map<string, number> {
  const indexByHash = new Map<string, number>();
  let fenceMarker: string | null = null;
  let blockLines: string[] = [];
  let blockIndex = 0;

  for (const line of content.split("\n")) {
    const fenceMatch = line.match(/^ {0,3}(`{3,}|~{3,})/);
    if (fenceMatch) {
      const marker = fenceMatch[1];
      if (!fenceMarker) {
        fenceMarker = marker;
        blockLines = [];
      } else if (marker[0] === fenceMarker[0] && marker.length >= fenceMarker.length) {
        const hash = stableHash(blockLines.join("\n"));
        if (!indexByHash.has(hash)) {
          indexByHash.set(hash, blockIndex);
        }
        blockIndex += 1;
        fenceMarker = null;
        blockLines = [];
      }
      continue;
    }
    if (fenceMarker) {
      blockLines.push(line);
    }
  }
  return indexByHash;
}

interface PythonRunContextValue {
  messageId?: string;
  streaming: boolean;
  onRunPython?: MarkdownRendererProps["onRunPython"];
  pythonRunRecords?: Record<string, PythonCodeRunRecord>;
  codeIndexByHash: Map<string, number>;
}

const PythonRunContext = createContext<PythonRunContextValue>({
  streaming: false,
  codeIndexByHash: new Map(),
});

function RunStatusBadge({ status }: { status: string }) {
  if (status === "running") {
    return <span className="python-inline-badge python-inline-badge--running">⟳ Running</span>;
  }
  if (status === "done") {
    return <span className="python-inline-badge python-inline-badge--done">✓ Done</span>;
  }
  if (status === "failed") {
    return <span className="python-inline-badge python-inline-badge--failed">✗ Failed</span>;
  }
  if (status === "stopped") {
    return <span className="python-inline-badge python-inline-badge--stopped">■ Stopped</span>;
  }
  return null;
}

function InlineRunOutput({ record }: { record: PythonCodeRunRecord }) {
  const stdout = record.stdout?.trim();
  const stderr = record.stderr?.trim();
  const isRunning = record.status === "running";

  if (!stdout && !stderr) {
    if (isRunning) {
      return (
        <div className="python-inline-output">
          <div className="python-inline-running">
            <span className="python-inline-spinner" />
            <span>Running…</span>
          </div>
        </div>
      );
    }
    return null;
  }

  return (
    <div className="python-inline-output">
      {stdout && (
        <pre className="python-inline-stdout">
          <code>{stdout}</code>
        </pre>
      )}
      {stderr && (
        <pre className="python-inline-stderr">
          <code>{stderr}</code>
        </pre>
      )}
    </div>
  );
}

/**
 * Custom streamdown renderer for python code blocks: streamdown's own
 * <CodeBlock> (Shiki highlighting + header + copy button) plus the Python
 * run button / status badge / inline output carried over from the legacy
 * MarkdownCodeBlock.
 */
function PythonCodeRenderer({ code, isIncomplete, language }: CustomRendererProps) {
  const { messageId, streaming, onRunPython, pythonRunRecords, codeIndexByHash } =
    useContext(PythonRunContext);
  const codeHash = stableHash(code);
  const codeBlockIndex = codeIndexByHash.get(codeHash) ?? 0;
  const record = messageId ? (pythonRunRecords?.[`${messageId}:${codeHash}`] ?? null) : null;
  const canRunPython = !streaming && !isIncomplete && Boolean(messageId) && Boolean(onRunPython);
  const showRunButton = canRunPython && !record;
  const isRunning = record?.status === "running";

  return (
    <>
      <CodeBlock code={code} language={language} isIncomplete={isIncomplete}>
        {record && <RunStatusBadge status={record.status} />}
        {showRunButton && (
          <button
            type="button"
            onClick={() => onRunPython?.({ messageId: messageId!, codeBlockIndex, code, codeHash })}
            title="运行 Python 代码"
          >
            <Play size={13} />
            Run
          </button>
        )}
        {isRunning && (
          <button type="button" disabled title="正在执行…">
            <Play size={13} />
            Running…
          </button>
        )}
        <CodeBlockCopyButton />
      </CodeBlock>
      {record && <InlineRunOutput record={record} />}
    </>
  );
}

/** Open http(s) links in the in-app browser when a handler is provided. */
function MarkdownLink({ href, children, ...props }: React.AnchorHTMLAttributes<HTMLAnchorElement>) {
  const openMarkdownLink = useMarkdownLinkHandler();
  const handleClick = (event: MouseEvent<HTMLAnchorElement>) => {
    const url = href ?? undefined;
    if (!openMarkdownLink || !isBrowserUrl(url)) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    void openMarkdownLink(url);
  };

  return (
    <a {...props} href={href} rel="noreferrer" onClick={handleClick}>
      {children}
    </a>
  );
}

// 模块级常量 so the memoized <Streamdown> never receives fresh
// prop identities on re-render.
const codePlugin = createCodePlugin({
  // 双主题一次输出：streamdown 经 `dark:` 变体随 <html>.dark 纯 CSS 切换。
  themes: [TEAL_LIGHT_THEME, TEAL_DARK_THEME],
});
// 默认的 `math` 预设关闭了单 `$` 行内公式（防货币误解析），模型输出普遍
// 使用 `$…$`，这里显式开启。
const mathPlugin = createMathPlugin({ singleDollarTextMath: true });
const streamdownPlugins = {
  code: codePlugin,
  math: mathPlugin,
  mermaid,
  renderers: [{ language: ["python", "py"], component: PythonCodeRenderer }],
};
// streamdown 的 rehypePlugins prop 会整体替换默认插件链（raw → sanitize →
// harden）。自定义链必须自带 rehypeRaw（缺失时 streamdown 会把 raw HTML
// 降级为纯文本），sanitize 使用与 react-markdown 管线共享的 chatSafeSchema
// ——默认 schema 的 src 白名单只有 http/https，会静默剥掉 chat-image:// 的
// <img src>（Agent 生成图因此在聊天气泡里渲染不出来）。不引入 rehype-harden：
// streamdown 默认 harden 配置为全放行（allowedProtocols:["*"] 等效空操作），
// sanitize 才是真正的闸门。
// ⚠️ 升级 streamdown 时必须复核其默认插件组成（raw/sanitize/harden 顺序与
// schema 默认值），确认此假设仍然成立。
type StreamdownRehypePlugins = NonNullable<
  React.ComponentProps<typeof Streamdown>["rehypePlugins"]
>;
const streamdownRehypePlugins = [
  rehypeRaw,
  [rehypeSanitize, chatSafeSchema],
] as unknown as StreamdownRehypePlugins;
const streamdownComponents: Components = {
  img: ({ src, alt }) => <MarkdownImage src={src} alt={alt} />,
  a: MarkdownLink as NonNullable<Components["a"]>,
};
const streamdownControls = {
  code: { copy: true, download: false },
  table: false,
} as const;
const streamdownLinkSafety = { enabled: false } as const;
const shikiTheme = [TEAL_LIGHT_THEME, TEAL_DARK_THEME] as [
  typeof TEAL_LIGHT_THEME,
  typeof TEAL_DARK_THEME,
];

export const MarkdownRenderer = memo(function MarkdownRenderer({
  content,
  streaming = false,
  messageId,
  onRunPython,
  pythonRunRecords,
  className,
}: MarkdownRendererProps) {
  const normalizedContent = useMemo(
    () =>
      normalizeSingleLineMathBlocks(
        normalizeLatexMathDelimiters(normalizeMathCodeFences(content)),
      ),
    [content],
  );
  const codeIndexByHash = useMemo(() => indexCodeBlocks(normalizedContent), [normalizedContent]);
  const pythonRunContext = useMemo<PythonRunContextValue>(
    () => ({ messageId, streaming, onRunPython, pythonRunRecords, codeIndexByHash }),
    [messageId, streaming, onRunPython, pythonRunRecords, codeIndexByHash],
  );
  const { containerProps: katexCopyProps, menuElement: katexCopyMenu } = useKatexCopy();

  return (
    <div
      className={cn("ai-streamdown text-[15px] leading-7 text-foreground", className)}
      {...katexCopyProps}
    >
      <PythonRunContext.Provider value={pythonRunContext}>
        <Streamdown
          mode={streaming ? "streaming" : "static"}
          isAnimating={streaming}
          caret={streaming ? "block" : undefined}
          plugins={streamdownPlugins}
          rehypePlugins={streamdownRehypePlugins}
          components={streamdownComponents}
          shikiTheme={shikiTheme}
          controls={streamdownControls}
          linkSafety={streamdownLinkSafety}
        >
          {normalizedContent}
        </Streamdown>
      </PythonRunContext.Provider>
      {katexCopyMenu}
    </div>
  );
});

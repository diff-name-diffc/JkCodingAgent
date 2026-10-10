import { createContext, memo, useContext, useMemo, useState } from "react";
import { Play } from "lucide-react";
import {
  CodeBlock,
  CodeBlockCopyButton,
  Streamdown,
  type Components,
  type CustomRendererProps,
} from "streamdown";
import { createMathPlugin } from "@streamdown/math";
import rehypeRaw from "rehype-raw";
import rehypeSanitize from "rehype-sanitize";
import "streamdown/styles.css";
import "katex/dist/katex.min.css";
import type { PythonCodeRunRecord } from "../../types";
import { cn } from "../../lib/cn";
import { stableHash } from "../../lib/stable-hash";
import { NEUTRAL_DARK_THEME, NEUTRAL_LIGHT_THEME } from "../../utils/shiki";
import { normalizeMarkdownMath } from "../../lib/normalize-math";
import { MarkdownImage } from "../markdown/MarkdownImage";
import { MarkdownLink } from "../markdown/MarkdownLink";
import { chatSafeSchema } from "../markdown/sanitize-schema";
import { useDeferredContent } from "../markdown/use-deferred-content";
import { StatusPill } from "../detail/StatusPill";
import { useKatexCopy } from "./katex-copy";
import { createChatCodePlugin } from "./shiki-code-plugin";
import { createLazyMermaidPlugin } from "./lazy-mermaid-plugin";

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
 * Input shaping (markdown/use-deferred-content.ts, shared with the legacy
 * pipeline): streaming segments are throttled to ~150ms before the
 * normalize+parse pass, and contents over 10KB render a plain-text
 * placeholder for the first frame only. Code highlighting uses our own shiki
 * plugin (shiki-code-plugin.ts — core engine + on-demand languages, replaces
 * @streamdown/code's full bundled registry); mermaid loads on first diagram
 * via lazy-mermaid-plugin.ts.
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

/** 无围栏时跳过代码块索引（python 运行记录映射用不到）。 */
function hasCodeFence(content: string): boolean {
  return content.includes("```") || content.includes("~~~");
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

const EMPTY_CODE_INDEX: Map<string, number> = new Map();

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
            <span>运行中…</span>
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
        {record && <StatusPill domain="python" status={record.status} />}
        {showRunButton && (
          <button
            type="button"
            onClick={() => onRunPython?.({ messageId: messageId!, codeBlockIndex, code, codeHash })}
            title="运行 Python 代码"
          >
            <Play size={13} />
            运行
          </button>
        )}
        {isRunning && (
          <button type="button" disabled title="正在执行…">
            <Play size={13} />
            运行中…
          </button>
        )}
        <CodeBlockCopyButton />
      </CodeBlock>
      {record && <InlineRunOutput record={record} />}
    </>
  );
}

// 模块级常量 so the memoized <Streamdown> never receives fresh
// prop identities on re-render.
// 高亮走自研插件（shiki-code-plugin.ts）：shiki core 单实例 + 13 语言按需
// 加载，双主题一次输出，streamdown 经 `dark:` 变体随 <html>.dark 纯 CSS 切换。
const codePlugin = createChatCodePlugin();
// mermaid 首个图出现时按需加载（lazy-mermaid-plugin.ts），核心包不进聊天 chunk。
const lazyMermaid = createLazyMermaidPlugin();
// 默认的 `math` 预设关闭了单 `$` 行内公式（防货币误解析），模型输出普遍
// 使用 `$…$`，这里显式开启。
const mathPlugin = createMathPlugin({ singleDollarTextMath: true });
const streamdownPlugins = {
  code: codePlugin,
  math: mathPlugin,
  mermaid: lazyMermaid,
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
const shikiTheme = [NEUTRAL_LIGHT_THEME, NEUTRAL_DARK_THEME] as [
  typeof NEUTRAL_LIGHT_THEME,
  typeof NEUTRAL_DARK_THEME,
];

export const MarkdownRenderer = memo(function MarkdownRenderer({
  content,
  streaming = false,
  messageId,
  onRunPython,
  pythonRunRecords,
  className,
}: MarkdownRendererProps) {
  // 已经开始流式渲染的正文继续使用同一分块树；只关闭动画与光标。
  // 切换为 static 会替换整段 DOM，工具轮切换时会丢失正在阅读的选区。
  const [startedStreaming] = useState(streaming);
  const { effectiveContent, deferred } = useDeferredContent(content, streaming);
  const normalizedContent = useMemo(
    () => (deferred ? effectiveContent : normalizeMarkdownMath(effectiveContent)),
    [effectiveContent, deferred],
  );
  const codeIndexByHash = useMemo(
    () =>
      deferred || !hasCodeFence(normalizedContent)
        ? EMPTY_CODE_INDEX
        : indexCodeBlocks(normalizedContent),
    [normalizedContent, deferred],
  );
  const pythonRunContext = useMemo<PythonRunContextValue>(
    () => ({ messageId, streaming, onRunPython, pythonRunRecords, codeIndexByHash }),
    [messageId, streaming, onRunPython, pythonRunRecords, codeIndexByHash],
  );
  const { containerProps: katexCopyProps, menuElement: katexCopyMenu } = useKatexCopy();

  if (deferred) {
    // 大文本首帧降级：先出纯文本让列表可滚动，rAF 后切换完整渲染。
    return (
      <div className={cn("ai-streamdown text-[15px] leading-7 text-foreground", className)}>
        <pre className="whitespace-pre-wrap break-words">{effectiveContent}</pre>
      </div>
    );
  }

  return (
    <div
      className={cn("ai-streamdown text-[15px] leading-7 text-foreground", className)}
      {...katexCopyProps}
    >
      <PythonRunContext.Provider value={pythonRunContext}>
        <Streamdown
          mode={streaming || startedStreaming ? "streaming" : "static"}
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

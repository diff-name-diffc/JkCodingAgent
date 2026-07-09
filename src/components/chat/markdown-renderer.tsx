import { MarkdownRenderer as LegacyMarkdownRenderer } from "../markdown/MarkdownRenderer";
import type { PythonCodeRunRecord } from "../../types";
import { cn } from "../../lib/cn";

/**
 * Thin Tailwind-friendly wrapper around the existing MarkdownRenderer.
 *
 * Why not rewrite? The legacy renderer already does the hard, fiddly work:
 *   - rehype-sanitize with a custom schema (chat-image://, asset://, data:,
 *     video/audio/details allowlist)
 *   - streaming throttle (~7fps) to survive 50 tokens/s
 *   - large-text deferral (>10k chars → rAF before full parse)
 *   - KaTeX math, GFM tables/task-lists, custom <img>/<a> handling
 *   - shiki code blocks via MarkdownCodeBlock
 *
 * Rewriting that here would risk regressions and duplicate the sanitize
 * surface — explicitly against the refactor's "don't break existing logic"
 * constraint. So this wrapper keeps the renderer and only standardizes the
 * surrounding shell + props.
 *
 * The CSS classes `markdown-surface` / `markdown-surface--chat` are defined
 * in App.css and style the rendered markdown (typography, code, tables).
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

export function MarkdownRenderer({
  content,
  streaming,
  messageId,
  onRunPython,
  pythonRunRecords,
  className,
}: MarkdownRendererProps) {
  return (
    <div className={cn("text-[15px] leading-7 text-foreground", className)}>
      <LegacyMarkdownRenderer
        content={content}
        variant="chat"
        streaming={streaming}
        messageId={messageId}
        onRunPython={onRunPython}
        pythonRunRecords={pythonRunRecords}
      />
    </div>
  );
}

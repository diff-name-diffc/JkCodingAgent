import * as React from "react";
import { ChevronDown } from "lucide-react";
import { cn } from "../../lib/cn";

export interface ReasoningBlockProps {
  text: string;
  elapsedMs: number;
  isStreaming?: boolean;
  className?: string;
}

export function ReasoningBlock({
  text,
  elapsedMs,
  isStreaming = false,
  className,
}: ReasoningBlockProps) {
  const [open, setOpen] = React.useState(false);
  const elapsed = elapsedMs > 0 ? `${(elapsedMs / 1000).toFixed(1)}s` : null;

  return (
    <div className={cn("ai-reasoning-block", className)}>
      <button
        type="button"
        className="ai-reasoning-trigger"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        <span className={cn("ai-reasoning-title", isStreaming && "ai-reasoning-shimmer")}>
          💭 思考过程
        </span>
        <span className="ml-auto flex items-center gap-1.5 text-[11px] text-muted-foreground">
          {isStreaming && <span>思考中…</span>}
          {elapsed && <span>{elapsed}</span>}
          <ChevronDown
            aria-hidden
            className={cn("h-3.5 w-3.5 transition-transform duration-fast", open && "rotate-180")}
          />
        </span>
      </button>

      {open && (
        <div className="chat-scroll max-h-[300px] overflow-y-auto border-t border-border/60 px-3 py-2.5 text-[13px] italic leading-relaxed text-muted-foreground">
          <p className="whitespace-pre-wrap break-words">{text}</p>
        </div>
      )}
    </div>
  );
}

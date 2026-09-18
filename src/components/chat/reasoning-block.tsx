import { useState } from "react";
import { ChevronDown } from "lucide-react";
import { cn } from "../../lib/cn";
import { usePersistedToggle } from "./row-ui-state";

export interface ReasoningBlockProps {
  text: string;
  elapsedMs: number;
  isStreaming?: boolean;
  /**
   * 流式阶段自动展开（用户未手动操作时生效）：思考进行中实时可见，
   * 正文开始输出后由调用方置 false 自动收起；用户点击后完全跟随用户。
   * 历史消息块不传，保持默认折叠。
   */
  autoOpen?: boolean;
  /** UI-24b-1：窗口化行卸载后思考块展开态经行级 store 恢复；流式气泡不传。 */
  persistKey?: string;
  className?: string;
}

export function ReasoningBlock({
  text,
  elapsedMs,
  isStreaming = false,
  autoOpen = false,
  persistKey,
  className,
}: ReasoningBlockProps) {
  const [open, setOpen] = usePersistedToggle(persistKey, false);
  // 用户交互后不再受 autoOpen 驱动（避免自动收起吃掉用户的手动展开）。
  const [userToggled, setUserToggled] = useState(false);
  const effectiveOpen = userToggled ? open : open || autoOpen;
  const elapsed = elapsedMs > 0 ? `${(elapsedMs / 1000).toFixed(1)}s` : null;

  return (
    <div className={cn("ai-reasoning-block", className)}>
      <button
        type="button"
        className="ai-reasoning-trigger"
        aria-expanded={effectiveOpen}
        onClick={() => {
          setUserToggled(true);
          setOpen((value) => !value);
        }}
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

      {effectiveOpen && (
        <div className="chat-scroll max-h-[300px] overflow-y-auto border-t border-border/60 px-3 py-2.5 text-[13px] italic leading-relaxed text-muted-foreground">
          <p className="whitespace-pre-wrap break-words">{text}</p>
        </div>
      )}
    </div>
  );
}

import { useState } from "react";
import { ChevronDown } from "lucide-react";
import { cn } from "../../lib/cn";
import { usePersistedToggle } from "./row-ui-state";

export interface ReasoningBlockProps {
  /** 模型思考链正文（reasoning / ` thinking` 标签）。可为空——仅有草稿时也展示。 */
  text: string;
  /**
   * 折叠进思考过程的中间推理草稿：助手在工具轮次间产出的、已被后续正文
   * 取代的文本段。按出现顺序展示在主思考链之后，不再作为独立块散落正文流。
   */
  drafts?: string[];
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
  drafts,
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
  const draftList = (drafts ?? []).filter((draft) => draft.trim().length > 0);

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
        {draftList.length > 0 && (
          <span className="ai-reasoning-meta">含 {draftList.length} 段中间推理</span>
        )}
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
          {text.trim() && <p className="whitespace-pre-wrap break-words">{text}</p>}
          {draftList.map((draft, index) => (
            <div key={index} className={cn("ai-reasoning-draft", text.trim() && "mt-2.5")}>
              <span className="ai-reasoning-draft-label">中间推理 {index + 1}</span>
              <p className="whitespace-pre-wrap break-words">{draft}</p>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

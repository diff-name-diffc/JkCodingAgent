import { memo, useEffect, useRef } from "react";
import { Brain, ChevronDown } from "lucide-react";
import { cn } from "../../lib/cn";
import { Button } from "../ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "../ui/collapsible";
import { usePersistedToggle, useRowUiStateContext } from "./row-ui-state";

export interface ReasoningBlockProps {
  /** 模型提供的思考正文，不包含面向用户的阶段性说明。 */
  text: string;
  elapsedMs: number;
  isStreaming?: boolean;
  /** 仅决定首次展示的展开态；正文开始或流式结束时保留用户正在阅读的内容。 */
  autoOpen?: boolean;
  /** 窗口化行卸载后恢复用户选择的展开态。 */
  persistKey?: string;
  className?: string;
}

export const ReasoningBlock = memo(function ReasoningBlock({
  text,
  elapsedMs,
  isStreaming = false,
  autoOpen = false,
  persistKey,
  className,
}: ReasoningBlockProps) {
  const [open, setOpen] = usePersistedToggle(persistKey, autoOpen);
  const rowUiState = useRowUiStateContext();
  const scrollRef = useRef<HTMLDivElement>(null);
  const followingRef = useRef(true);
  const elapsed = elapsedMs > 0 ? `${(elapsedMs / 1000).toFixed(1)}s` : null;

  // 流式自动展开是初始默认值而非用户选择，不写 store；虚拟化卸载重挂载后
  // autoOpen 已随流式结束变 false，自动展开会静默退回折叠。趁 autoOpen 生效
  // 且用户尚未手动收起（store 无记录）时播种 true，让自动展开与手动展开
  // 一样可跨重挂载恢复。
  useEffect(() => {
    if (autoOpen && persistKey !== undefined && rowUiState) {
      if (rowUiState.get(persistKey) === undefined) {
        rowUiState.set(persistKey, true);
      }
    }
  }, [autoOpen, persistKey, rowUiState]);

  useEffect(() => {
    const element = scrollRef.current;
    if (open && isStreaming && followingRef.current && element) {
      element.scrollTop = element.scrollHeight;
    }
  }, [text, open, isStreaming]);

  return (
    <Collapsible open={open} onOpenChange={setOpen} className={cn("ai-reasoning-block", className)}>
      <CollapsibleTrigger asChild>
        <Button
          variant="ghost"
          type="button"
          className="ai-reasoning-trigger h-auto min-h-9 justify-start rounded-none px-3 py-2"
        >
          <Brain aria-hidden className="text-muted-foreground" />
          <span className="ai-reasoning-title">模型思考</span>
          <span className="ml-auto flex items-center gap-2 text-[11px] font-normal text-muted-foreground">
            {isStreaming && <span>思考中…</span>}
            {elapsed && <span title="思考用时">{elapsed}</span>}
            <ChevronDown
              aria-hidden
              className={cn(
                "transition-transform duration-fast motion-reduce:transition-none",
                open && "rotate-180",
              )}
            />
          </span>
        </Button>
      </CollapsibleTrigger>

      <CollapsibleContent>
        <div
          ref={scrollRef}
          tabIndex={0}
          role="region"
          aria-label="模型思考内容"
          onScroll={(event) => {
            const element = event.currentTarget;
            followingRef.current =
              element.scrollHeight - element.scrollTop - element.clientHeight <= 24;
          }}
          className="chat-scroll max-h-48 overflow-y-auto overscroll-contain border-t border-border/60 px-3 py-2.5 text-[13px] leading-relaxed text-muted-foreground focus-visible:outline focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-[var(--border-focus)]"
        >
          <p className="m-0 whitespace-pre-wrap break-words">{text}</p>
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
});

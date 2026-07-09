import * as React from "react";
import { ArrowUp, Loader2, Square } from "lucide-react";
import type { DispatcherModelConfig } from "../../types";
import { cn } from "../../lib/cn";
import { Button } from "../ui/button";
import { Textarea } from "../ui/textarea";
import { Tooltip, TooltipContent, TooltipTrigger } from "../ui/tooltip";
import { ModelSelector } from "./model-selector";

/**
 * Composer mode mirrors the existing send/stop/resume pipeline contract.
 */
export type ComposerMode = "send" | "stop" | "resume";

export interface PromptInputProps {
  value: string;
  onValueChange: (value: string) => void;
  mode: ComposerMode;
  onSend: () => void;
  onStop: () => void;
  /** Called on Enter when mode === "resume" and the input is empty. */
  onResume?: () => void;
  models?: DispatcherModelConfig[];
  onSelectModel?: (modelIndex: number) => void;
  placeholder?: string;
  disabled?: boolean;
  className?: string;
  /** Slot for extra trailing controls (attachment, voice, shortcuts). */
  leadingSlot?: React.ReactNode;
  trailingSlot?: React.ReactNode;
}

const MAX_HEIGHT = 240;

/**
 * Bottom-anchored prompt input for the refactored chat surface.
 *
 * Keyboard:
 *   Enter        → send (or stop-resume flow when mode==="resume" + empty)
 *   Shift+Enter  → newline
 *   IME composition is respected (the parent app's useComposedInput already
 *   handles this; we replicate the core check here for the new surface).
 */
export function PromptInput({
  value,
  onValueChange,
  mode,
  onSend,
  onStop,
  onResume,
  models,
  onSelectModel,
  placeholder = "输入消息…  (Enter 发送 / Shift+Enter 换行)",
  disabled,
  className,
  leadingSlot,
  trailingSlot,
}: PromptInputProps) {
  const textareaRef = React.useRef<HTMLTextAreaElement | null>(null);
  const composingRef = React.useRef(false);

  // Auto-grow the textarea up to MAX_HEIGHT.
  React.useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, MAX_HEIGHT)}px`;
  }, [value]);

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // Respect IME composition — don't hijack the Enter that confirms a CJK
    // candidate. isComposing covers most browsers; the app also tracks a
    // composition flag via useComposedInput for older WebKit.
    if (e.nativeEvent.isComposing || composingRef.current) return;

    if (e.key !== "Enter" || e.shiftKey) return;
    e.preventDefault();

    if (mode === "stop") return;
    if (mode === "resume" && !value.trim()) {
      onResume?.();
      return;
    }
    if (mode === "send" && value.trim()) {
      onSend();
    }
  };

  const canSend = mode === "send" && value.trim().length > 0 && !disabled;

  return (
    <div
      className={cn(
        "ai-prompt-terminal mx-auto w-full max-w-[920px] rounded-2xl border border-border bg-card p-2 shadow-soft",
        className,
      )}
    >
      <Textarea
        ref={textareaRef}
        value={value}
        onChange={(e) => onValueChange(e.target.value)}
        onKeyDown={handleKeyDown}
        onCompositionStart={() => (composingRef.current = true)}
        onCompositionEnd={() => (composingRef.current = false)}
        placeholder={placeholder}
        disabled={disabled}
        rows={1}
        aria-label="消息输入框"
        className="ai-prompt-textarea max-h-[240px] resize-none border-0 bg-transparent px-2 py-1.5 text-[15px] leading-6 shadow-none focus-visible:ring-0"
      />

      <div className="ai-prompt-toolbar flex items-center gap-1 px-1 pt-1">
        {leadingSlot}
        {models && onSelectModel && (
          <ModelSelector models={models} onSelect={onSelectModel} />
        )}
        <div className="flex-1" />
        {trailingSlot}

        {mode === "stop" ? (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="destructive"
                size="icon"
                aria-label="停止生成"
                onClick={onStop}
              >
                <Square className="h-4 w-4" fill="currentColor" />
              </Button>
            </TooltipTrigger>
            <TooltipContent>停止生成</TooltipContent>
          </Tooltip>
        ) : (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                size="icon"
                aria-label="发送消息"
                disabled={!canSend}
                onClick={onSend}
              >
                {disabled ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : (
                  <ArrowUp className="h-4 w-4" />
                )}
              </Button>
            </TooltipTrigger>
            <TooltipContent>发送 (Enter)</TooltipContent>
          </Tooltip>
        )}
      </div>
    </div>
  );
}

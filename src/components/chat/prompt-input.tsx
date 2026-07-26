import * as React from "react";
import { ArrowUp, Loader2, Paperclip, Square, X } from "lucide-react";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { ImageSegment, ModelLibraryEntry } from "../../types";
import { cn } from "../../lib/cn";
import { isImeComposing } from "../../utils";
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
  /** Images already saved to chat-images and staged for the next send. */
  attachments?: ImageSegment[];
  /** Receive image Files from paste / file picker; parent persists them. */
  onAttachImages?: (files: File[]) => void;
  onRemoveAttachment?: (id: string) => void;
  models?: ModelLibraryEntry[];
  activeEntryId?: string;
  activeLabel?: string;
  onSelectModel?: (entryId: string) => void;
  placeholder?: string;
  disabled?: boolean;
  editing?: boolean;
  onCancelEdit?: () => void;
  className?: string;
  /** Slot for extra trailing controls (attachment, voice, shortcuts). */
  leadingSlot?: React.ReactNode;
  trailingSlot?: React.ReactNode;
}

const LINE_HEIGHT = 24;
const MAX_ROWS = 8;
const MAX_HEIGHT = LINE_HEIGHT * MAX_ROWS + 12;

function imageFilesFromClipboard(data: DataTransfer | null): File[] {
  if (!data) return [];
  const files: File[] = [];
  for (const item of Array.from(data.items)) {
    if (item.kind === "file" && item.type.startsWith("image/")) {
      const file = item.getAsFile();
      if (file) files.push(file);
    }
  }
  return files;
}

/**
 * Bottom-anchored prompt input for the refactored chat surface.
 *
 * Keyboard:
 *   Enter        → send (or stop-resume flow when mode==="resume" + empty)
 *   Shift+Enter  → newline
 *   IME composition is respected.
 *
 * Images can be staged by pasting from the clipboard or via the paperclip
 * picker; staged images render as a thumbnail strip above the editor.
 */
export function PromptInput({
  value,
  onValueChange,
  mode,
  onSend,
  onStop,
  onResume,
  attachments = [],
  onAttachImages,
  onRemoveAttachment,
  models,
  activeEntryId,
  activeLabel,
  onSelectModel,
  placeholder = "输入消息…",
  disabled,
  editing = false,
  onCancelEdit,
  className,
  leadingSlot,
  trailingSlot,
}: PromptInputProps) {
  const textareaRef = React.useRef<HTMLTextAreaElement | null>(null);
  const fileInputRef = React.useRef<HTMLInputElement | null>(null);

  // Auto-grow the textarea up to MAX_HEIGHT.
  React.useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, MAX_HEIGHT)}px`;
  }, [value]);

  const hasContent = value.trim().length > 0 || attachments.length > 0;

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (isImeComposing(e) || e.key !== "Enter" || e.shiftKey) return;
    e.preventDefault();

    if (mode === "stop") return;
    if (mode === "resume" && !hasContent) {
      onResume?.();
      return;
    }
    if (mode === "send" && hasContent) {
      onSend();
    }
  };

  const handlePaste = (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const files = imageFilesFromClipboard(e.clipboardData);
    if (files.length === 0) return; // plain-text paste keeps default behavior
    e.preventDefault();
    onAttachImages?.(files);
  };

  const handleFilePicked = (e: React.ChangeEvent<HTMLInputElement>) => {
    const files = Array.from(e.target.files ?? []).filter((f) => f.type.startsWith("image/"));
    e.target.value = ""; // allow picking the same file again
    if (files.length > 0) onAttachImages?.(files);
  };

  const canSend = mode === "send" && hasContent && !disabled;

  return (
    <div className={cn("ai-chat-column", className)}>
      <div className="ai-prompt-terminal rounded-2xl border border-border bg-card p-2">
        {editing && (
          <div className="ai-prompt-editing">
            <span>正在编辑消息</span>
            <button type="button" onClick={onCancelEdit} aria-label="取消编辑">
              <X className="h-3.5 w-3.5" />
              取消
            </button>
          </div>
        )}
        {attachments.length > 0 && (
          <div className="flex flex-wrap gap-2 px-2 pb-1 pt-1">
            {attachments.map((image) => (
              <div
                key={image.id}
                className="group relative h-16 w-16 overflow-hidden rounded-md border border-border"
              >
                <img
                  src={convertFileSrc(image.path)}
                  alt={image.alt || "附件图片"}
                  className="h-full w-full object-cover"
                />
                <button
                  type="button"
                  aria-label="移除图片"
                  onClick={() => onRemoveAttachment?.(image.id)}
                  className="absolute right-0.5 top-0.5 flex h-4 w-4 items-center justify-center rounded-full bg-black/60 text-white opacity-0 transition-opacity group-hover:opacity-100"
                >
                  <X className="h-3 w-3" />
                </button>
              </div>
            ))}
          </div>
        )}
        <div className="ai-prompt-editor">
          <Textarea
            ref={textareaRef}
            value={value}
            onChange={(e) => onValueChange(e.target.value)}
            onKeyDown={handleKeyDown}
            onPaste={handlePaste}
            placeholder={placeholder}
            disabled={disabled}
            rows={1}
            aria-label="消息输入框"
            className="ai-prompt-textarea min-h-11 max-h-[204px] resize-none border-0 bg-transparent px-2 py-2 text-[15px] leading-6 shadow-none focus-visible:ring-0"
          />
        </div>

        <div className="ai-prompt-toolbar flex items-center gap-1 px-1 pt-1">
          <input
            ref={fileInputRef}
            type="file"
            accept="image/*"
            multiple
            hidden
            onChange={handleFilePicked}
          />
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon"
                className="ai-attachment-button"
                aria-label="添加图片"
                onClick={() => fileInputRef.current?.click()}
                type="button"
              >
                <Paperclip className="h-4 w-4" />
              </Button>
            </TooltipTrigger>
            <TooltipContent>添加图片（也可直接粘贴截图）</TooltipContent>
          </Tooltip>
          {leadingSlot}
          {models && onSelectModel && (
            <ModelSelector
              models={models}
              activeEntryId={activeEntryId}
              activeLabel={activeLabel}
              onSelect={onSelectModel}
            />
          )}
          <div className="flex-1" />
          {trailingSlot}

          {mode === "stop" ? (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="destructive"
                  size="icon"
                  className="ai-composer-action is-stop"
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
                  className="ai-composer-action is-send"
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
      <div
        className="mt-1.5 flex justify-end px-2 text-[11px] leading-4 text-muted-foreground/70"
        aria-hidden="true"
      >
        Enter 发送 · Shift+Enter 换行 · 支持粘贴截图
      </div>
    </div>
  );
}

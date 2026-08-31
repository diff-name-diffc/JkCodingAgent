import { motion } from "framer-motion";
import { Pencil } from "lucide-react";
import type { DispatcherMessage } from "../../types";
import { cn } from "../../lib/cn";
import { ChatAvatar } from "./chat-avatar";
import { ActionButton } from "./message-actions";
import { MarkdownImage } from "../markdown/MarkdownImage";

/**
 * User message bubble for the refactored chat surface.
 *
 * Right-aligned, soft accent-tinted bubble with a compact identity anchor. Renders the user's
 * text content; image segments are rendered as thumbnails below the text via MarkdownImage,
 * which resolves `chat-image://` ids through the backend and supports click-to-enlarge.
 *
 * Hovering the row reveals an edit action below the bubble. The parent owns the edit lifecycle:
 * populate the composer first, then truncate and resend only after the user confirms.
 */
export interface UserMessageProps {
  message: DispatcherMessage;
  onEdit?: (message: DispatcherMessage) => void;
  className?: string;
}

export function UserMessage({ message, onEdit, className }: UserMessageProps) {
  // 防御性守卫：归一化保证 segments 必然存在，?? [] 只是兜底防脏载荷打崩列表。
  const segments = message.segments ?? [];
  const text = message.content?.trim() ?? "";
  const images = segments.filter((s) => s.type === "image");

  return (
    <motion.div
      initial={{ opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.18, ease: [0.2, 0.8, 0.2, 1] }}
      className={cn("ai-user-message flex items-start justify-end gap-3", className)}
    >
      <div className="flex max-w-[72%] flex-col items-end">
        <div className="ai-user-bubble rounded-[18px] rounded-br-[4px] px-4 py-3 text-[15px] leading-7">
          {text && <p className="whitespace-pre-wrap break-words">{text}</p>}
          {images.length > 0 && (
            <div className="mt-2 flex flex-wrap gap-2">
              {images.map((img) => (
                <MarkdownImage
                  key={img.id}
                  src={`chat-image://${img.imageId}`}
                  alt={img.alt || "附件图片"}
                />
              ))}
            </div>
          )}
        </div>
        {onEdit && (
          <div className="ai-message-actions">
            <ActionButton label="编辑并重新发送" onClick={() => onEdit(message)}>
              <Pencil className="h-4 w-4" />
            </ActionButton>
          </div>
        )}
      </div>
      <ChatAvatar role="user" className="mt-0.5" />
    </motion.div>
  );
}

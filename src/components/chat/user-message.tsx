import { motion } from "framer-motion";
import type { DispatcherMessage } from "../../types";
import { cn } from "../../lib/cn";

/**
 * User message bubble for the refactored chat surface.
 *
 * Right-aligned, soft accent-tinted bubble, no avatar. Renders the user's
 * text content. Image segments are rendered as thumbnails below the text
 * (the heavy lifting — chat-image:// protocol, paste handling — stays in the
 * existing pipeline; here we only display already-persisted segments).
 */
export interface UserMessageProps {
  message: DispatcherMessage;
  className?: string;
}

export function UserMessage({ message, className }: UserMessageProps) {
  // `segments` may be undefined when a message slipped past normalization
  // (e.g. legacy rows without segmentsJson, or an unnormalized load path).
  // Guard here so a malformed row never crashes the whole message list.
  const segments = message.segments ?? [];
  const text = message.content?.trim() ?? "";
  const images = segments.filter((s) => s.type === "image");

  return (
    <motion.div
      initial={{ opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.18, ease: [0.2, 0.8, 0.2, 1] }}
      className={cn("ai-user-message flex items-start justify-end", className)}
    >
      <div className="ai-user-bubble max-w-[72%] rounded-[18px] rounded-br-[4px] px-4 py-3 text-[15px] leading-7">
        {text && <p className="whitespace-pre-wrap break-words">{text}</p>}
        {images.length > 0 && (
          <div className="mt-2 flex flex-wrap gap-2">
            {images.map((img) => (
              <img
                key={img.id}
                src={`chat-image://${img.imageId}`}
                alt={img.alt || "attached image"}
                className="max-h-48 rounded-md border border-border object-cover"
              />
            ))}
          </div>
        )}
      </div>
    </motion.div>
  );
}

import * as React from "react";
import { ArrowDown } from "lucide-react";
import { AnimatePresence, motion } from "framer-motion";
import { Button } from "../ui/button";

/**
 * Sentinel + "jump to latest" button for the message list.
 *
 * Place a <ChatScrollAnchor /> at the very bottom of the message list. It:
 *   - acts as the scroll target when the user clicks "jump to latest"
 *   - renders a floating button when the view is scrolled up, calling
 *     `onJumpToLatest` (which should call the parent's scrollToBottom)
 *
 * The pinned/unpinned state is owned by useAutoScroll in the parent; this
 * component is purely presentational based on the `showJumpButton` prop.
 */
export interface ChatScrollAnchorProps {
  showJumpButton: boolean;
  onJumpToLatest: () => void;
  /** Unread streaming-chunk count badge, optional. */
  unreadCount?: number;
}

export function ChatScrollAnchor({
  showJumpButton,
  onJumpToLatest,
  unreadCount,
}: ChatScrollAnchorProps) {
  const ref = React.useRef<HTMLDivElement | null>(null);

  return (
    <>
      <div ref={ref} aria-hidden className="h-px w-full shrink-0" />
      <AnimatePresence>
        {showJumpButton && (
          <motion.div
            initial={{ opacity: 0, y: 6 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: 6 }}
            transition={{ duration: 0.15 }}
            className="pointer-events-none absolute inset-x-0 bottom-3 flex justify-center"
          >
            <Button
              variant="outline"
              size="sm"
              className="pointer-events-auto gap-1.5 rounded-full border-border bg-card/90 px-3 shadow-medium backdrop-blur"
              onClick={onJumpToLatest}
              aria-label="跳转到最新"
            >
              <ArrowDown className="h-3.5 w-3.5" />
              最新
              {unreadCount ? (
                <span className="ml-1 rounded-full bg-primary px-1.5 py-0.5 text-[10px] leading-none text-primary-foreground">
                  {unreadCount}
                </span>
              ) : null}
            </Button>
          </motion.div>
        )}
      </AnimatePresence>
    </>
  );
}

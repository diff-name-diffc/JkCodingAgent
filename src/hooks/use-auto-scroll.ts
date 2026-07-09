import { useCallback, useEffect, useRef, useState } from "react";

/**
 * Auto-scroll-to-bottom that respects the user's scroll position.
 *
 * Behaviour:
 *   - While the user is parked at (or near) the bottom, new content pushes the
 *     view down — ideal for streaming AI replies.
 *   - The moment the user scrolls up to read history, auto-follow stops, so a
 *     long streaming reply never yanks them back to the bottom.
 *   - An external caller can force a scroll-to-bottom (e.g. when switching
 *     conversations or clicking a "jump to latest" button) via `scrollToBottom`.
 *
 * Returns a ref to attach to the scroll container, a `pinned` flag (true while
 * following), and a `scrollToBottom` action.
 */
export interface AutoScrollApi {
  /** Attach to the scrolling element. */
  containerRef: React.RefObject<HTMLDivElement | null>;
  /** True while the view is parked at the bottom (auto-follow active). */
  pinned: boolean;
  /** Force-scroll to the bottom, ignoring current pin state. */
  scrollToBottom: (opts?: { behavior?: ScrollBehavior }) => void;
  /** Re-check whether we're at the bottom (call after layout shifts). */
  recompute: () => void;
}

const BOTTOM_THRESHOLD_PX = 56;

export function useAutoScroll(): AutoScrollApi {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [pinned, setPinned] = useState(true);
  // Track pin state in a ref so the scroll handler (which closes over it) can
  // read the latest value without re-subscribing on every change.
  const pinnedRef = useRef(true);
  pinnedRef.current = pinned;

  const recompute = useCallback(() => {
    const el = containerRef.current;
    if (!el) return;
    const distanceFromBottom =
      el.scrollHeight - el.scrollTop - el.clientHeight;
    const isAtBottom = distanceFromBottom <= BOTTOM_THRESHOLD_PX;
    setPinned((prev) => (prev !== isAtBottom ? isAtBottom : prev));
  }, []);

  const scrollToBottom = useCallback(
    (opts?: { behavior?: ScrollBehavior }) => {
      const el = containerRef.current;
      if (!el) return;
      el.scrollTo({
        top: el.scrollHeight,
        behavior: opts?.behavior ?? "auto",
      });
      setPinned(true);
      pinnedRef.current = true;
    },
    [],
  );

  // Attach the native scroll listener once.
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const onScroll = () => {
      const distanceFromBottom =
        el.scrollHeight - el.scrollTop - el.clientHeight;
      const isAtBottom = distanceFromBottom <= BOTTOM_THRESHOLD_PX;
      if (isAtBottom !== pinnedRef.current) {
        pinnedRef.current = isAtBottom;
        setPinned(isAtBottom);
      }
    };

    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, []);

  // Follow layout/content changes while pinned.
  useEffect(() => {
    if (pinnedRef.current) {
      const el = containerRef.current;
      if (el) el.scrollTop = el.scrollHeight;
    }
  });

  return { containerRef, pinned, scrollToBottom, recompute };
}

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
  containerRef: (element: HTMLDivElement | null) => void;
  /** True while the view is parked at the bottom (auto-follow active). */
  pinned: boolean;
  /** Force-scroll to the bottom, ignoring current pin state. */
  scrollToBottom: (opts?: { behavior?: ScrollBehavior }) => void;
  /** Re-check whether we're at the bottom (call after layout shifts). */
  recompute: () => void;
}

const BOTTOM_THRESHOLD_PX = 56;

export function useAutoScroll(sessionId: string | null): AutoScrollApi {
  const elementRef = useRef<HTMLDivElement | null>(null);
  const [element, setElement] = useState<HTMLDivElement | null>(null);
  const [pinned, setPinned] = useState(true);
  // Track pin state in a ref so the scroll handler (which closes over it) can
  // read the latest value without re-subscribing on every change.
  const pinnedRef = useRef(true);
  pinnedRef.current = pinned;

  const containerRef = useCallback((next: HTMLDivElement | null) => {
    elementRef.current = next;
    setElement(next);
  }, []);

  const setFollowing = useCallback((following: boolean) => {
    pinnedRef.current = following;
    setPinned((current) => (current === following ? current : following));
  }, []);

  const recompute = useCallback(() => {
    const el = elementRef.current;
    if (!el) return;
    const distanceFromBottom =
      el.scrollHeight - el.scrollTop - el.clientHeight;
    const isAtBottom = distanceFromBottom <= BOTTOM_THRESHOLD_PX;
    setFollowing(isAtBottom);
  }, [setFollowing]);

  const scrollToBottom = useCallback(
    (opts?: { behavior?: ScrollBehavior }) => {
      const el = elementRef.current;
      if (!el) return;
      el.scrollTo({
        top: el.scrollHeight,
        behavior: opts?.behavior ?? "auto",
      });
      setFollowing(true);
    },
    [setFollowing],
  );

  // Manual upward intent wins immediately, even inside the bottom threshold.
  useEffect(() => {
    const el = element;
    if (!el) return;

    let lastScrollTop = el.scrollTop;
    let touchY: number | null = null;

    const onScroll = () => {
      const movedUp = el.scrollTop < lastScrollTop - 0.5;
      lastScrollTop = el.scrollTop;
      if (movedUp) {
        setFollowing(false);
        return;
      }
      const distanceFromBottom =
        el.scrollHeight - el.scrollTop - el.clientHeight;
      if (distanceFromBottom <= BOTTOM_THRESHOLD_PX && !pinnedRef.current) {
        setFollowing(true);
      }
    };

    const onWheel = (event: WheelEvent) => {
      if (event.deltaY < 0) setFollowing(false);
    };

    const onTouchStart = (event: TouchEvent) => {
      touchY = event.touches[0]?.clientY ?? null;
    };

    const onTouchMove = (event: TouchEvent) => {
      const nextY = event.touches[0]?.clientY ?? null;
      if (touchY !== null && nextY !== null && nextY > touchY) {
        setFollowing(false);
      }
      touchY = nextY;
    };

    el.addEventListener("scroll", onScroll, { passive: true });
    el.addEventListener("wheel", onWheel, { passive: true });
    el.addEventListener("touchstart", onTouchStart, { passive: true });
    el.addEventListener("touchmove", onTouchMove, { passive: true });
    return () => {
      el.removeEventListener("scroll", onScroll);
      el.removeEventListener("wheel", onWheel);
      el.removeEventListener("touchstart", onTouchStart);
      el.removeEventListener("touchmove", onTouchMove);
    };
  }, [element, setFollowing]);

  // Follow actual layout growth rather than every React render.
  useEffect(() => {
    if (!element) return;
    let frame: number | null = null;
    const follow = () => {
      if (!pinnedRef.current) return;
      if (frame !== null) cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        element.scrollTop = element.scrollHeight;
        frame = null;
      });
    };
    const observer = new ResizeObserver(follow);
    observer.observe(element);
    if (element.firstElementChild) observer.observe(element.firstElementChild);
    follow();
    return () => {
      observer.disconnect();
      if (frame !== null) cancelAnimationFrame(frame);
    };
  }, [element]);

  useEffect(() => {
    setFollowing(true);
    requestAnimationFrame(() => {
      const current = elementRef.current;
      if (current) current.scrollTop = current.scrollHeight;
    });
  }, [sessionId, setFollowing]);

  return { containerRef, pinned, scrollToBottom, recompute };
}

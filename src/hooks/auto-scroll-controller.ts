/** 只容忍浏览器的小数像素误差；靠近底部不等于用户已经回到底部。 */
const BOTTOM_TOLERANCE_PX = 2;
const MOVEMENT_TOLERANCE_PX = 0.5;

export interface AutoScrollController {
  isFollowing: () => boolean;
  adjustScrollPosition: (scroll: () => void) => void;
  destroy: () => void;
}

/** 内层详情能接管此方向的滚动时，不改变外层消息列表的跟随状态。 */
function scrollBelongsToNestedContent(
  element: HTMLElement,
  target: EventTarget | null,
  direction: number,
): boolean {
  let current = target instanceof Element ? target : null;
  while (current && current !== element) {
    const style = getComputedStyle(current);
    if (/(auto|scroll)/.test(style.overflowY) && current.scrollHeight > current.clientHeight) {
      const canScroll =
        direction < 0
          ? current.scrollTop > 0
          : current.scrollTop + current.clientHeight < current.scrollHeight;
      if (canScroll || /^(contain|none)$/.test(style.overscrollBehaviorY)) return true;
    }
    current = current.parentElement;
  }
  return false;
}

/**
 * 同步状态独立于 React 渲染：输入事件优先取消待执行帧，RAF 执行前再次校验。
 * ResizeObserver 只负责内容/视口变化，scroll 只负责用户位置，两者不互相抢写。
 */
export function attachAutoScroll(element: HTMLDivElement): AutoScrollController {
  let following = true;
  let frame: number | null = null;
  let lastScrollTop = element.scrollTop;
  let lastMaximum = element.scrollHeight - element.clientHeight;
  let touchY: number | null = null;
  let draggingScrollbar = false;
  const maximum = () => Math.max(0, element.scrollHeight - element.clientHeight);
  // WebKit 回弹可能暂时越过边界，回弹复位不能被当作用户反向滚动。
  const position = () => Math.max(0, Math.min(element.scrollTop, maximum()));
  const atBottom = () => maximum() - element.scrollTop <= BOTTOM_TOLERANCE_PX;

  const synchronizeScrollPosition = () => {
    lastScrollTop = position();
    lastMaximum = maximum();
  };
  const cancelFollow = () => {
    if (frame !== null) cancelAnimationFrame(frame);
    frame = null;
  };
  const detach = () => {
    following = false;
    cancelFollow();
  };
  const movedUp = () => position() < lastScrollTop - MOVEMENT_TOLERANCE_PX;
  // 虚拟测量可能先缩短总高、钳低 scrollTop，再在 scroll 事件前增高一部分。
  // 此时已不贴当前底部，仍属于布局收缩；上滑输入会提前 detach，不走此分支。
  const layoutShrank = () => following && maximum() < lastMaximum - MOVEMENT_TOLERANCE_PX;

  const follow = () => {
    if (!following || draggingScrollbar || frame !== null) return;
    frame = requestAnimationFrame(() => {
      frame = null;
      if (!following || draggingScrollbar) return;
      // 拖动滚动条可能早于 scroll 事件发生；未派发事件的位置变化也必须优先。
      if (movedUp() && !layoutShrank()) {
        detach();
        synchronizeScrollPosition();
        return;
      }
      element.scrollTop = maximum();
      synchronizeScrollPosition();
    });
  };

  const onScroll = () => {
    const upward = movedUp();
    const downward = position() > lastScrollTop + MOVEMENT_TOLERANCE_PX;
    const layoutClamped = layoutShrank();
    synchronizeScrollPosition();
    if (upward && !layoutClamped) detach();
    else if (downward && atBottom() && !draggingScrollbar) {
      following = true;
      follow();
    }
  };
  const onIntent = (target: EventTarget | null, direction: number) => {
    if (scrollBelongsToNestedContent(element, target, direction)) return;
    if (direction < 0) detach();
    else if (atBottom() && !draggingScrollbar) {
      following = true;
      follow();
    }
  };
  const onWheel = (event: WheelEvent) => {
    if (event.ctrlKey || Math.abs(event.deltaY) <= Math.abs(event.deltaX)) return;
    onIntent(event.target, event.deltaY);
  };
  const onTouchStart = (event: TouchEvent) => {
    touchY = event.touches.length === 1 ? event.touches[0].clientY : null;
  };
  const onTouchMove = (event: TouchEvent) => {
    const nextY = event.touches.length === 1 ? event.touches[0].clientY : null;
    if (touchY !== null && nextY !== null && nextY !== touchY) {
      onIntent(event.target, touchY - nextY);
    }
    touchY = nextY;
  };
  const onTouchEnd = () => {
    touchY = null;
  };
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey) return;
    if (
      event.target instanceof Element &&
      event.target.closest(
        "input, textarea, select, button, a[href], [contenteditable]:not([contenteditable='false']), [role='textbox']",
      )
    ) {
      return;
    }
    if (
      ["ArrowUp", "PageUp", "Home"].includes(event.key) ||
      (event.key === " " && event.shiftKey)
    ) {
      onIntent(event.target, -1);
    } else if (["ArrowDown", "PageDown", "End", " "].includes(event.key)) {
      onIntent(event.target, 1);
    }
  };
  const onPointerDown = (event: PointerEvent) => {
    // 原生滚动条的事件目标是滚动容器本身；正文内点选/展开不打断跟随。
    if (event.target === element && event.pointerType === "mouse") {
      draggingScrollbar = true;
      detach();
    }
  };
  const onPointerUp = () => {
    if (!draggingScrollbar) return;
    draggingScrollbar = false;
    synchronizeScrollPosition();
    following = atBottom();
    follow();
  };

  element.addEventListener("scroll", onScroll, { passive: true });
  element.addEventListener("wheel", onWheel, { passive: true });
  element.addEventListener("touchstart", onTouchStart, { passive: true });
  element.addEventListener("touchmove", onTouchMove, { passive: true });
  element.addEventListener("touchend", onTouchEnd, { passive: true });
  element.addEventListener("touchcancel", onTouchEnd, { passive: true });
  element.addEventListener("keydown", onKeyDown);
  element.addEventListener("pointerdown", onPointerDown, { passive: true });
  element.ownerDocument.addEventListener("pointerup", onPointerUp, { passive: true });
  element.ownerDocument.addEventListener("pointercancel", onPointerUp, { passive: true });
  const observer = new ResizeObserver(follow);
  observer.observe(element);
  if (element.firstElementChild) observer.observe(element.firstElementChild);
  follow();

  return {
    isFollowing: () => following,
    adjustScrollPosition: (scroll) => {
      // 测量补偿前先接收可能尚未派发 scroll 的用户位移，再忽略补偿本身。
      onScroll();
      scroll();
      synchronizeScrollPosition();
      follow();
    },
    destroy: () => {
      following = false;
      cancelFollow();
      observer.disconnect();
      element.removeEventListener("scroll", onScroll);
      element.removeEventListener("wheel", onWheel);
      element.removeEventListener("touchstart", onTouchStart);
      element.removeEventListener("touchmove", onTouchMove);
      element.removeEventListener("touchend", onTouchEnd);
      element.removeEventListener("touchcancel", onTouchEnd);
      element.removeEventListener("keydown", onKeyDown);
      element.removeEventListener("pointerdown", onPointerDown);
      element.ownerDocument.removeEventListener("pointerup", onPointerUp);
      element.ownerDocument.removeEventListener("pointercancel", onPointerUp);
    },
  };
}

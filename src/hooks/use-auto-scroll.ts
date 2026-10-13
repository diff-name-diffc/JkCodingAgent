import { useCallback, useEffect, useRef, useState } from "react";
import { attachAutoScroll, type AutoScrollController } from "./auto-scroll-controller";

/** 消息列表触底后跟随增长；上滑立即脱离，会话切换重新定位到底部。 */
export function useAutoScroll(sessionId: string | null) {
  const [element, setElement] = useState<HTMLDivElement | null>(null);
  const controllerRef = useRef<AutoScrollController | null>(null);

  const containerRef = useCallback((next: HTMLDivElement | null) => {
    setElement(next);
  }, []);

  // 虚拟列表的行高补偿不是用户滚动，不改变跟随状态。
  const adjustScrollPosition = useCallback((scroll: () => void) => {
    if (controllerRef.current) controllerRef.current.adjustScrollPosition(scroll);
    else scroll();
  }, []);

  useEffect(() => {
    if (!element) return;
    const controller = attachAutoScroll(element);
    controllerRef.current = controller;
    return () => {
      controller.destroy();
      controllerRef.current = null;
    };
  }, [element, sessionId]);

  return { containerRef, adjustScrollPosition };
}

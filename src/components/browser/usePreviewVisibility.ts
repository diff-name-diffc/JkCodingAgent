import { useEffect, useState, type RefObject } from "react";

export function observePreviewVisibility(
  element: HTMLElement,
  onChange: (visible: boolean) => void,
) {
  let active = true;
  let observer: IntersectionObserver | null = null;
  const update = () => {
    if (!active) return;
    if (document.visibilityState !== "visible") {
      observer?.disconnect();
      observer = null;
      onChange(false);
      return;
    }
    if (observer) return;
    const current = new IntersectionObserver((entries) => {
      // 文档再次显示时更换观察者，上一轮排队的交集回调不能重新启用采集。
      if (!active || observer !== current) return;
      const entry = entries.find((candidate) => candidate.target === element);
      if (!entry) return;
      onChange(
        entry.isIntersecting &&
          entry.intersectionRatio > 0 &&
          document.visibilityState === "visible",
      );
    });
    observer = current;
    onChange(false);
    observer.observe(element);
  };
  document.addEventListener("visibilitychange", update);
  update();
  return () => {
    active = false;
    observer?.disconnect();
    observer = null;
    document.removeEventListener("visibilitychange", update);
  };
}

/** 只有文档可见且监控区域实际进入视口时，才允许订阅/解码画面。 */
export function usePreviewVisibility(containerRef: RefObject<HTMLElement | null>): boolean {
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    const element = containerRef.current;
    if (!element) return;
    return observePreviewVisibility(element, setVisible);
  }, [containerRef]);
  return visible;
}

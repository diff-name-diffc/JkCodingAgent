import { useEffect, useRef, type RefObject } from "react";
import { isImeKeyEvent } from "../lib/keyboard-bindings";

/** 可聚焦元素选择器（保守集合，覆盖本仓库自研覆盖层的全部交互元素）。 */
export const FOCUSABLE_SELECTOR = [
  "a[href]",
  "button:not([disabled])",
  "textarea:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
].join(",");

/**
 * 自研覆盖层的焦点陷阱与还原（UI-23b）。
 *
 * active 期间：聚焦 initialFocus()（缺省为容器内首个可聚焦元素），Tab /
 * Shift+Tab 在容器内循环；关闭时把焦点还原到打开前的元素——但仅当焦点仍在
 * 容器内或已回到 body（用户点击外部等合法移动焦点后不抢夺）。
 *
 * 仅用于自研覆盖层（命令面板 / MCP 状态弹窗 / 新分类对话框 / KaTeX 菜单）；
 * Radix Dialog/Sheet 自带焦点管理，不接本 hook。IME 组合期不拦截 Tab。
 */
export function useFocusTrap(
  active: boolean,
  containerRef: RefObject<HTMLElement | null>,
  options?: { initialFocus?: () => HTMLElement | null },
): void {
  const initialFocusRef = useRef(options?.initialFocus);
  initialFocusRef.current = options?.initialFocus;

  useEffect(() => {
    if (!active) return;
    const container = containerRef.current;
    if (!container) return;

    const restoreTarget = (document.activeElement as HTMLElement | null) ?? null;
    const focusables = () =>
      Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR));
    const initial = initialFocusRef.current?.() ?? null;
    (initial ?? focusables()[0] ?? container).focus();

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Tab" || isImeKeyEvent(event)) return;
      const items = focusables();
      if (items.length === 0) {
        event.preventDefault();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      const current = document.activeElement;
      if (event.shiftKey) {
        if (current === first || !container.contains(current)) {
          event.preventDefault();
          last.focus();
        }
      } else if (current === last || !container.contains(current)) {
        event.preventDefault();
        first.focus();
      }
    };
    // capture 阶段拦截，先于容器内局部 onKeyDown 的默认 Tab 行为。
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      const current = document.activeElement;
      const focusMovedLegitimately =
        current !== null && current !== document.body && !container.contains(current);
      if (!focusMovedLegitimately) restoreTarget?.focus();
    };
  }, [active, containerRef]);
}

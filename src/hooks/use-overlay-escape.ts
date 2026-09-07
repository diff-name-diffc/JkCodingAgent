import { useEffect, useRef } from "react";
import { isTopOverlay, peekOverlay, popOverlay, pushOverlay, shouldHandleEscape } from "../lib/overlay-stack";

/**
 * 自研覆盖层的统一 Escape 接线（UI-23b）。
 *
 * active 期间把 id 压入全局覆盖层栈（cleanup 保证弹出）；window keydown 中
 * 仅当自己是栈顶且事件未被更内层（Radix 弹层等）处理时响应——「嵌套弹层
 * Escape 只处理最顶层」。底层快捷键（use-chat-shortcuts / GraphPanel 等）
 * 依据 hasOpenOverlay() 让路，消除一次按键同时关闭多层的「双关」。
 *
 * Radix 弹层不进本栈：它们走 DismissableLayer 自己的分支栈，与本栈覆盖层
 * 共存时由 defaultPrevented 检查兜底让路。
 */
export function useOverlayEscape(id: string, active: boolean, onClose: () => void): void {
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  useEffect(() => {
    if (!active) return;
    pushOverlay(id);
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (!shouldHandleEscape(peekOverlay(), id, event.defaultPrevented)) return;
      // 双保险：非栈顶不响应（shouldHandleEscape 已判定，此处防御并发变更）。
      if (!isTopOverlay(id)) return;
      event.preventDefault();
      closeRef.current();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      popOverlay(id);
    };
  }, [active, id]);
}

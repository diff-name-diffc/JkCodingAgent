import { useCallback } from "react";
import type { MouseEvent } from "react";

/** 选中达到该字符数（去首尾空白后）即自动复制。 */
const MIN_AUTO_COPY_LENGTH = 3;

/**
 * 返回一个 mouseup 处理器：鼠标在聊天消息区内完成拖选后，若选中文本达到
 * 3 个字符则自动写入剪贴板。挂在包裹消息的滚动容器上即可。
 */
export function useCopyOnSelect() {
  return useCallback((event: MouseEvent<HTMLElement>) => {
    if (event.button !== 0) return;
    const container = event.currentTarget;
    // 等浏览器完成本次选区更新后再读取。
    window.setTimeout(() => {
      const selection = window.getSelection();
      if (!selection || selection.isCollapsed) return;
      if (!container.contains(selection.anchorNode) || !container.contains(selection.focusNode)) {
        return;
      }
      const text = selection.toString();
      // 用码点计数，避免 emoji 等多码元字符被重复计数。
      if ([...text.trim()].length < MIN_AUTO_COPY_LENGTH) return;
      navigator.clipboard.writeText(text).catch(() => {
        // 剪贴板权限被拒等场景静默失败，用户仍可按快捷键复制。
      });
    }, 0);
  }, []);
}

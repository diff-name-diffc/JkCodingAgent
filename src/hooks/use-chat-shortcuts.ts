import { useRef } from "react";
import { hasOpenOverlay } from "../lib/overlay-stack";
import { RADIX_MODAL_OPEN_SELECTOR, type ShortcutBinding } from "../lib/keyboard-bindings";
import { useGlobalShortcuts } from "./use-global-shortcuts";

export interface ChatShortcutHandlers {
  onToggleCommandPalette?: () => void;
  onNewConversation?: () => void;
  onToggleSidebar?: () => void;
  onFocusPrompt?: () => void;
  onCloseArtifact?: () => void;
  /** Mod+Shift+A：开/关 Artifact 详情面板（UI-23d，无选中详情时由回调自行 no-op）。 */
  onToggleArtifactPanel?: () => void;
}

/**
 * Global keyboard shortcuts for the chat surface.
 *
 *   Mod+K        — toggle the command palette
 *   Mod+N        — new conversation
 *   Mod+B        — toggle sidebar
 *   Mod+L        — focus the prompt input
 *   Mod+Shift+A  — toggle the artifact/detail panel (UI-23d)
 *   Esc          — if a self-managed overlay is open, let the top overlay
 *                  handle it; otherwise close the artifact panel
 *
 * UI-23a：判定逻辑（Mod 平台映射 / IME 组合期 / 终端与 Monaco 让路 / 输入
 * 目标豁免）在 lib/keyboard-bindings 纯函数层；注册与 enabled 门控委托给
 * useGlobalShortcuts（bindings 走 ref，隐藏工作区不注册）。
 */
export function useChatShortcuts(
  handlers: ChatShortcutHandlers,
  options?: { enabled?: boolean },
) {
  const handlersRef = useRef(handlers);
  handlersRef.current = handlers;

  const bindings: ShortcutBinding[] = [
    { key: "k", mod: true, handler: () => handlersRef.current.onToggleCommandPalette?.() },
    { key: "n", mod: true, handler: () => handlersRef.current.onNewConversation?.() },
    { key: "b", mod: true, handler: () => handlersRef.current.onToggleSidebar?.() },
    { key: "l", mod: true, handler: () => handlersRef.current.onFocusPrompt?.() },
    { key: "a", mod: true, shift: true, handler: () => handlersRef.current.onToggleArtifactPanel?.() },
    { key: "Escape", mod: false, preventDefault: false, handler: (event: KeyboardEvent) => {
      // 自研覆盖层（命令面板/执行图等）打开时让路：由栈顶覆盖层自己的
      // Escape 处理，避免一次按键同时关掉覆盖层与 Artifact 面板。
      if (hasOpenOverlay()) return;
      // Radix 弹层（设置 Dialog / Sheet 抽屉 / 下拉菜单 / Select）不进自研栈，
      // 且其 Escape 处理不 preventDefault——按 DOM 存在性让路（UI-23b，锚点
      // 与 Mod 键跨栈让路共用 RADIX_MODAL_OPEN_SELECTOR 单一出处）。
      if (document.querySelector(RADIX_MODAL_OPEN_SELECTOR)) {
        return;
      }
      event.preventDefault();
      handlersRef.current.onCloseArtifact?.();
    } },
  ];

  useGlobalShortcuts(bindings, options);
}

import { useEffect, useRef } from "react";
import { hasOpenOverlay } from "../lib/overlay-stack";
import {
  isMacPlatform,
  matchesBinding,
  shouldSkipBinding,
  type ShortcutBinding,
} from "../lib/keyboard-bindings";

export interface ChatShortcutHandlers {
  onToggleCommandPalette?: () => void;
  onNewConversation?: () => void;
  onToggleSidebar?: () => void;
  onFocusPrompt?: () => void;
  onCloseArtifact?: () => void;
}

/**
 * Global keyboard shortcuts for the chat surface.
 *
 *   Mod+K   — toggle the command palette
 *   Mod+N   — new conversation
 *   Mod+B   — toggle sidebar
 *   Mod+L   — focus the prompt input
 *   Esc     — if a self-managed overlay is open, let the top overlay handle it;
 *             otherwise close the artifact panel
 *
 * UI-23a：判定逻辑（Mod 平台映射 / IME 组合期 / 终端与 Monaco 让路 / 输入目标
 * 豁免）在 lib/keyboard-bindings 纯函数层；handlers 走 ref 存最新值，effect 仅
 * 依赖 enabled——调用方传字面量对象不会导致 window 监听器每次渲染重挂。
 * enabled=false（多项目保活下的隐藏工作区）完全不注册，消除多实例叠加触发。
 */
export function useChatShortcuts(
  handlers: ChatShortcutHandlers,
  options?: { enabled?: boolean },
) {
  const enabled = options?.enabled ?? true;
  const handlersRef = useRef(handlers);
  handlersRef.current = handlers;

  useEffect(() => {
    if (!enabled) return;
    const mac = isMacPlatform();
    const bindings: ShortcutBinding[] = [
      { key: "k", mod: true, handler: () => handlersRef.current.onToggleCommandPalette?.() },
      { key: "n", mod: true, handler: () => handlersRef.current.onNewConversation?.() },
      { key: "b", mod: true, handler: () => handlersRef.current.onToggleSidebar?.() },
      { key: "l", mod: true, handler: () => handlersRef.current.onFocusPrompt?.() },
      { key: "Escape", mod: false, preventDefault: false, handler: (event: KeyboardEvent) => {
        // 自研覆盖层（命令面板/执行图等）打开时让路：由栈顶覆盖层自己的
        // Escape 处理，避免一次按键同时关掉覆盖层与 Artifact 面板。
        if (hasOpenOverlay()) return;
        event.preventDefault();
        handlersRef.current.onCloseArtifact?.();
      } },
    ];

    const onKey = (event: KeyboardEvent) => {
      // 已被内层处理（Radix 弹层、局部 onKeyDown 等）的按键不再劫持。
      if (event.defaultPrevented) return;
      for (const binding of bindings) {
        if (!matchesBinding(event, binding, mac)) continue;
        if (shouldSkipBinding(event, binding, mac)) continue;
        if (binding.preventDefault !== false) event.preventDefault();
        binding.handler(event);
        break;
      }
    };

    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [enabled]);
}

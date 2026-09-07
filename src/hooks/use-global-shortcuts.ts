import { useEffect, useRef } from "react";
import {
  isMacPlatform,
  matchesBinding,
  shouldSkipBinding,
  type ShortcutBinding,
} from "../lib/keyboard-bindings";

/**
 * 通用全局快捷键注册（UI-23d）：判定与让路规则全部走 lib/keyboard-bindings
 * 纯函数层（IME 组合期 / 终端与 Monaco 让路 / 输入目标豁免 / defaultPrevented
 * 早退）。bindings 走 ref 存最新值，effect 仅依赖 enabled——调用方传字面量
 * 数组不会导致 window 监听器每次渲染重挂；enabled=false（多项目保活下的
 * 隐藏工作区）完全不注册。
 */
export function useGlobalShortcuts(
  bindings: ShortcutBinding[],
  options?: { enabled?: boolean },
): void {
  const enabled = options?.enabled ?? true;
  const bindingsRef = useRef(bindings);
  bindingsRef.current = bindings;

  useEffect(() => {
    if (!enabled) return;
    const mac = isMacPlatform();
    const onKey = (event: KeyboardEvent) => {
      // 已被内层处理（Radix 弹层、局部 onKeyDown 等）的按键不再劫持。
      if (event.defaultPrevented) return;
      for (const binding of bindingsRef.current) {
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

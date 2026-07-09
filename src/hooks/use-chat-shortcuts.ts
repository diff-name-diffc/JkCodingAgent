import { useEffect } from "react";

interface ShortcutBinding {
  /** Mod means Cmd on macOS, Ctrl elsewhere. */
  key: string;
  mod?: boolean;
  shift?: boolean;
  alt?: boolean;
  handler: (event: KeyboardEvent) => void;
  /** Prevent default browser behavior when matched. */
  preventDefault?: boolean;
}

function isMac(): boolean {
  if (typeof navigator === "undefined") return false;
  return /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
}

function matches(event: KeyboardEvent, binding: ShortcutBinding): boolean {
  if (binding.mod !== undefined) {
    const wantsMod = binding.mod;
    const hasMod = isMac() ? event.metaKey : event.ctrlKey;
    if (wantsMod !== hasMod) return false;
  }
  if (binding.shift !== undefined && binding.shift !== event.shiftKey) return false;
  if (binding.alt !== undefined && binding.alt !== event.altKey) return false;
  // Normalize key to lowercase for letter keys; keep symbols as-is.
  const key = event.key.length === 1 ? event.key.toLowerCase() : event.key;
  return key === binding.key;
}

/**
 * Global keyboard shortcuts for the chat surface.
 *
 *   Mod+K   — toggle the command palette (reserved for a future palette)
 *   Mod+N   — new conversation
 *   Mod+B   — toggle sidebar
 *   Mod+L   — focus the prompt input
 *   Esc     — if a palette/dialog is open, let it handle its own escape;
 *             otherwise close the artifact panel
 */
export function useChatShortcuts(bindings: {
  onToggleCommandPalette?: () => void;
  onNewConversation?: () => void;
  onToggleSidebar?: () => void;
  onFocusPrompt?: () => void;
  onCloseArtifact?: () => void;
}) {
  useEffect(() => {
    const handlers: ShortcutBinding[] = [
      { key: "k", mod: true, handler: () => bindings.onToggleCommandPalette?.() },
      { key: "n", mod: true, handler: () => bindings.onNewConversation?.() },
      { key: "b", mod: true, handler: () => bindings.onToggleSidebar?.() },
      { key: "l", mod: true, handler: () => bindings.onFocusPrompt?.() },
      { key: "Escape", handler: () => bindings.onCloseArtifact?.() },
    ].filter((b) => b.key !== undefined) as ShortcutBinding[];

    const onKey = (event: KeyboardEvent) => {
      // Don't hijack typing inside inputs/textareas UNLESS a mod key is held.
      const target = event.target as HTMLElement | null;
      const isTypingTarget =
        target &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.isContentEditable);
      for (const binding of handlers) {
        if (!matches(event, binding)) continue;
        if (isTypingTarget && !binding.mod) continue;
        if (binding.preventDefault !== false) event.preventDefault();
        binding.handler(event);
        break;
      }
    };

    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [bindings]);
}

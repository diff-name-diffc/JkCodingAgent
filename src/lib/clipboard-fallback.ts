/**
 * 剪贴板写入的唯一入口与 WebKit 兜底链。
 *
 * macOS Tauri 的 WKWebView 里，前端两条复制路径都不可靠：
 * - 异步 Clipboard API 在缺少 WebView 焦点/用户激活判定时抛 NotAllowedError；
 * - execCommand("copy") 必须在用户事件处理器同步执行，且在 Radix 弹层
 *   （ContextMenu/DropdownMenu，modal 默认开启 FocusScope focus trap）的
 *   onSelect 期间，兜底 textarea 的 focus() 会被 focus trap 同步拉回菜单内，
 *   选区进不了 textarea——复制静默落空（右键菜单「复制路径」失灵的根因）。
 *
 * 因此 copyTextToClipboard 走「原生优先」降级链：
 *   1. Tauri clipboard 插件（原生写剪贴板，不受 WebView 焦点/激活/focus trap
 *      影响，也覆盖生产环境自定义协议下 navigator.clipboard 可能缺失的场景）；
 *   2. execCommand("copy") 同步兜底（纯浏览器 dev 环境，需用户手势栈；
 *      选区与焦点在兜底后还原，避免破坏聊天区拖选自动复制 use-copy-on-select）；
 *   3. 原生异步 Clipboard API（最后手段）。
 * 三条路都失败才向上抛错，保持调用方现有 catch 语义。
 */
import { writeText as tauriClipboardWriteText } from "@tauri-apps/plugin-clipboard-manager";

let nativeWriteText: ((text: string) => Promise<void>) | null = null;

export async function copyTextToClipboard(text: string): Promise<void> {
  try {
    await tauriClipboardWriteText(text);
    return;
  } catch {
    // 非 Tauri 环境（纯浏览器 dev / 测试）或插件不可用：继续降级。
  }

  if (copyViaExecCommand(text)) {
    return;
  }

  if (nativeWriteText) {
    await nativeWriteText(text);
    return;
  }

  throw new Error("Clipboard API not available");
}

export function installClipboardWriteFallback(): void {
  if (typeof document === "undefined" || typeof navigator === "undefined") {
    return;
  }

  const clipboard = navigator.clipboard;
  if (clipboard && typeof clipboard.writeText === "function") {
    nativeWriteText = clipboard.writeText.bind(clipboard);
    clipboard.writeText = (text: string): Promise<void> => copyTextToClipboard(text);
    return;
  }

  // Clipboard API 整体缺失的极端环境：直接替换为统一入口（尽力而为）。
  try {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: (text: string): Promise<void> => copyTextToClipboard(text),
      },
    });
  } catch {
    // navigator.clipboard 只读且不可覆盖：维持现状，调用方各自兜底。
  }
}

function copyViaExecCommand(text: string): boolean {
  if (typeof document.execCommand !== "function") {
    return false;
  }
  const previousActive =
    document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const selection = window.getSelection();
  const savedRanges =
    selection && selection.rangeCount > 0
      ? Array.from({ length: selection.rangeCount }, (_, i) => selection.getRangeAt(i).cloneRange())
      : [];

  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "");
  textarea.style.cssText = "position:fixed;top:-9999px;left:-9999px;opacity:0;";
  document.body.appendChild(textarea);

  let copied = false;
  try {
    textarea.focus();
    textarea.select();
    textarea.setSelectionRange(0, text.length);
    copied = document.execCommand("copy");
  } catch {
    // copied 保持初始 false
  } finally {
    textarea.remove();
  }

  previousActive?.focus();
  if (selection) {
    selection.removeAllRanges();
    for (const range of savedRanges) {
      selection.addRange(range);
    }
  }
  return copied;
}

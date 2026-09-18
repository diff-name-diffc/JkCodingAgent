/**
 * navigator.clipboard.writeText 的 WebKit 兜底安装器。
 *
 * macOS Tauri 的 WKWebView（以及部分内嵌 WebView 场景）里，异步 Clipboard API
 * 会在缺少 WebView 焦点/用户激活判定时抛 NotAllowedError——聊天代码块、mermaid
 * 块的复制按钮（streamdown 内部实现）与 KaTeX 点击复制都会因此「点了没反应」。
 * execCommand("copy") 同步复制在 WebKit 手势栈内仍可用，但它必须在用户事件
 * 处理器同步执行——promise 回调里补调会被拒绝。因此这里采用「同步优先」次序：
 * 先在调用现场（点击处理器内）尝试 execCommand，成功即返回；失败（个别环境
 * 不支持）再回退原生异步 API。两条路都失败才向上抛错，保持调用方现有 catch
 * 语义。选区与焦点在兜底后还原，避免破坏聊天区拖选自动复制（use-copy-on-select）。
 */
export function installClipboardWriteFallback(): void {
  if (typeof document === "undefined" || typeof navigator === "undefined") {
    return;
  }

  const clipboard = navigator.clipboard;
  if (clipboard && typeof clipboard.writeText === "function") {
    const nativeWriteText = clipboard.writeText.bind(clipboard);
    clipboard.writeText = (text: string): Promise<void> => {
      if (copyViaExecCommand(text)) {
        return Promise.resolve();
      }
      return nativeWriteText(text);
    };
    return;
  }

  // Clipboard API 整体缺失的极端环境：直接替换为 execCommand 实现（尽力而为）。
  try {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: (text: string): Promise<void> => {
          if (!copyViaExecCommand(text)) {
            return Promise.reject(new Error("Clipboard API not available"));
          }
          return Promise.resolve();
        },
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

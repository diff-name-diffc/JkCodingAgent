import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";

interface BrowserLinkNavOptions {
  activeSessionId: string | null;
  projectPath: string | null;
  /** 用户主动点击链接后的打开面板回调（仅用户手势触发，Agent 执行不再自动弹出）。 */
  onOpen: () => void;
}

async function runBrowserCommand(command: string, args: Record<string, unknown>): Promise<boolean> {
  try {
    await invoke(command, args);
    return true;
  } catch (error) {
    console.error(`${command} 执行失败:`, error);
    return false;
  }
}

/**
 * 会话浏览器的链接导航（原 useBrowserSessionDock 瘦身）：
 * 无头化改造后浏览器执行细节不再自动弹出（也不再有最小化/停靠窗口），
 * 这里只保留用户主动点击聊天链接 → 会话浏览器内导航 → 打开浏览器标签的路径。
 */
export function useBrowserSessionLinkNav({ activeSessionId, projectPath, onOpen }: BrowserLinkNavOptions) {
  const openUrl = useCallback(async () => {
    if (!activeSessionId) return;
    // 链接 URL 由调用方经 markdown 链接点击路径先行导航（browser_navigate），
    // 此处仅负责打开浏览器标签视图；保留命令封装以维持单一调用形态。
    onOpen();
  }, [activeSessionId, onOpen]);

  const navigateToUrl = useCallback(
    async (url: string) => {
      if (!activeSessionId) return;
      const succeeded = await runBrowserCommand("browser_navigate", {
        sessionId: activeSessionId,
        url,
        projectPath,
      });
      if (succeeded) onOpen();
    },
    [activeSessionId, onOpen, projectPath],
  );

  return { openUrl, navigateToUrl };
}

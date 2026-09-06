import { useCallback, useRef, useState } from "react";
import type { MouseEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm, open as openDialog } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { BrowserStatus } from "../../types";
import { normalizeBrowserUrlInput } from "../../lib/browser-url";
import type { BrowserPanelSession } from "./useBrowserPanelSession";

interface BrowserProfileImportResult {
  profileName: string;
  targetPath: string;
}

interface BrowserProfileCandidate {
  profileName: string;
  path: string;
  userDataRoot: string;
}

interface UseBrowserPanelCommandsOptions {
  sessionId: string | null;
  projectPath?: string;
  session: BrowserPanelSession;
  onMinimize?: () => void | Promise<void>;
  onReopen?: () => void | Promise<void>;
}

/**
 * 浏览器面板动作层（UI-18 自 BrowserPanel 拆出）：busy 单飞锁 + 全部命令。
 * 每个 invoke 继续携带 projectPath —— 后端 resolve_project_path /
 * validate_project_workspace 权限闸门不变（验收条款「不绕过既有权限行为」）。
 */
export function useBrowserPanelCommands({
  sessionId,
  projectPath,
  session,
  onMinimize,
  onReopen,
}: UseBrowserPanelCommandsOptions) {
  const { setStatus, setError, appendLog, refreshStatus } = session;
  const [busy, setBusy] = useState(false);
  const submittingUrlRef = useRef(false);

  const runBrowserAction = useCallback(
    async (action: () => Promise<void>, options: { refresh?: boolean } = {}) => {
      if (busy) return;
      setBusy(true);
      setError(null);
      try {
        await action();
        if (options.refresh !== false) {
          await refreshStatus();
        }
      } catch (reason) {
        setError(String(reason));
      } finally {
        setBusy(false);
      }
    },
    [busy, refreshStatus, setError],
  );

  const startBrowser = useCallback(async () => {
    if (!sessionId) return;
    await runBrowserAction(async () => {
      const next = await invoke<BrowserStatus>("browser_start", {
        sessionId,
        projectPath: projectPath || null,
      });
      setStatus(next);
    }, { refresh: false });
  }, [projectPath, runBrowserAction, sessionId, setStatus]);

  const stopBrowser = useCallback(async () => {
    if (!sessionId) return;
    await runBrowserAction(async () => {
      await invoke("browser_stop", { sessionId });
    });
  }, [runBrowserAction, sessionId]);

  const goBack = useCallback(async () => {
    if (!sessionId) return;
    await runBrowserAction(async () => {
      await invoke("browser_go_back", {
        sessionId,
        projectPath: projectPath || null,
      });
    });
  }, [projectPath, runBrowserAction, sessionId]);

  const reloadPage = useCallback(async () => {
    if (!sessionId) return;
    await runBrowserAction(async () => {
      await invoke("browser_reload", {
        sessionId,
        projectPath: projectPath || null,
      });
    });
  }, [projectPath, runBrowserAction, sessionId]);

  /**
   * 地址栏导航：裸输入归一化（补 https:// 等），Enter 与 blur 双提交的
   * 防重由 submittingUrlRef 承担（与拆分前一致）。
   */
  const navigateTo = useCallback(
    async (rawUrl: string) => {
      if (!sessionId || submittingUrlRef.current) return;
      const url = rawUrl.trim();
      if (!url || url === "about:blank") return;
      const normalizedUrl = normalizeBrowserUrlInput(url);
      submittingUrlRef.current = true;
      try {
        await runBrowserAction(async () => {
          await invoke("browser_navigate", {
            sessionId,
            url: normalizedUrl,
            projectPath: projectPath || null,
          });
        });
      } finally {
        window.setTimeout(() => {
          submittingUrlRef.current = false;
        }, 0);
      }
    },
    [projectPath, runBrowserAction, sessionId],
  );

  const importChromeProfile = useCallback(async () => {
    if (!sessionId || busy) return;

    let candidates: BrowserProfileCandidate[] = [];
    let scanMessage: string;
    try {
      candidates = await invoke<BrowserProfileCandidate[]>(
        "browser_list_chrome_profile_candidates",
      );
      if (candidates.length > 0) {
        const visibleCandidates = candidates
          .slice(0, 6)
          .map(
            (candidate, index) =>
              `${index + 1}. ${candidate.profileName}: ${candidate.path}`,
          );
        const hiddenCount = candidates.length - visibleCandidates.length;
        scanMessage = [
          "",
          "已扫描到常见 Chrome Profile，可在目录选择器中直接选择：",
          ...visibleCandidates,
          hiddenCount > 0 ? `另有 ${hiddenCount} 个候选路径未显示。` : "",
        ]
          .filter(Boolean)
          .join("\n");
      } else {
        scanMessage = "\n未在常见位置找到 Chrome Profile，可继续手动选择。";
      }
    } catch (reason) {
      scanMessage = `\n扫描常见 Chrome Profile 失败：${String(reason)}\n仍可继续手动选择。`;
    }

    const confirmed = await confirm(
      [
        "请先完全退出 Google Chrome。继续后会把所选 Chrome Profile 复制到当前项目的浏览器副本中，CloakBrowser 只读写这个副本。",
        scanMessage,
      ].join(""),
      {
        title: "导入 Chrome 登录态",
        kind: "warning",
      },
    );
    if (!confirmed) return;

    const selected = await openDialog({
      directory: true,
      multiple: false,
      title: "选择 Chrome Profile 目录（Default / Profile 1）或 Chrome 用户数据根目录",
      defaultPath: candidates[0]?.path,
    });
    if (!selected || Array.isArray(selected)) return;

    await runBrowserAction(async () => {
      const result = await invoke<BrowserProfileImportResult>("browser_import_chrome_profile", {
        sessionId,
        projectPath: projectPath || null,
        chromeProfilePath: selected,
      });
      appendLog(`已导入 Chrome Profile：${result.profileName} → ${result.targetPath}`);
      const next = await invoke<BrowserStatus>("browser_start", {
        sessionId,
        projectPath: projectPath || null,
      });
      setStatus(next);
    }, { refresh: false });
  }, [appendLog, busy, projectPath, runBrowserAction, sessionId, setStatus]);

  const openCurrentUrl = useCallback(async () => {
    const url = session.status?.url?.trim();
    if (!url || url === "about:blank") return;
    try {
      await openUrl(url);
    } catch (reason) {
      setError(String(reason));
    }
  }, [session.status?.url, setError]);

  const minimizeBrowser = useCallback(async () => {
    if (!sessionId || !onMinimize) return;
    await runBrowserAction(async () => {
      await onMinimize();
    });
  }, [onMinimize, runBrowserAction, sessionId]);

  const reopenBrowser = useCallback(async () => {
    if (!sessionId || !onReopen) return;
    await runBrowserAction(async () => {
      await onReopen();
    });
  }, [onReopen, runBrowserAction, sessionId]);

  /** 画布点击 → 页面坐标：映射基于实时 rect 与画布后备尺寸，窗口/面板缩放安全。 */
  const handleCanvasClick = useCallback(
    async (event: MouseEvent<HTMLCanvasElement>) => {
      if (!sessionId) return;
      const canvas = event.currentTarget;
      const rect = canvas.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0 || canvas.width <= 0 || canvas.height <= 0) {
        return;
      }

      const x = ((event.clientX - rect.left) / rect.width) * canvas.width;
      const y = ((event.clientY - rect.top) / rect.height) * canvas.height;

      await runBrowserAction(async () => {
        await invoke("browser_click_at", {
          sessionId,
          projectPath: projectPath || null,
          x,
          y,
        });
      });
    },
    [projectPath, runBrowserAction, sessionId],
  );

  return {
    busy,
    runBrowserAction,
    startBrowser,
    stopBrowser,
    goBack,
    reloadPage,
    navigateTo,
    importChromeProfile,
    openCurrentUrl,
    minimizeBrowser,
    reopenBrowser,
    handleCanvasClick,
  };
}

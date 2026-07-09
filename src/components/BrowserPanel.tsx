import { useCallback, useEffect, useRef, useState } from "react";
import type { KeyboardEvent, MouseEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { confirm, open as openDialog } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  ArrowLeft,
  ExternalLink,
  KeyRound,
  Maximize2,
  Minimize2,
  Monitor,
  MonitorDown,
  MonitorUp,
  Power,
  RefreshCw,
  Square,
  X,
} from "lucide-react";
import type { BrowserFrameEvent, BrowserLogEvent, BrowserStatus } from "../types";

interface BrowserProfileImportResult {
  profileName: string;
  targetPath: string;
}

interface BrowserProfileCandidate {
  profileName: string;
  path: string;
  userDataRoot: string;
}

interface Props {
  sessionId: string | null;
  projectPath?: string;
  width: number;
  active: boolean;
  expanded?: boolean;
  onToggleExpanded?: () => void;
  onClose?: () => void;
  onMinimize?: () => void | Promise<void>;
  onReopen?: () => void | Promise<void>;
}

export function BrowserPanel({
  sessionId,
  projectPath,
  width,
  active,
  expanded = false,
  onToggleExpanded,
  onClose,
  onMinimize,
  onReopen,
}: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [status, setStatus] = useState<BrowserStatus | null>(null);
  const [logs, setLogs] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const submittingUrlRef = useRef(false);

  const imageRef = useRef<HTMLImageElement>(new Image());

  const drawFrame = useCallback((frame: BrowserFrameEvent) => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const image = imageRef.current;
    image.onload = () => {
      canvas.width = frame.width || image.width;
      canvas.height = frame.height || image.height;
      ctx.drawImage(image, 0, 0, canvas.width, canvas.height);
    };
    image.src = frame.data;
  }, []);

  const refreshStatus = useCallback(async () => {
    if (!sessionId) return;
    try {
      const next = await invoke<BrowserStatus>("browser_get_status", { sessionId });
      setStatus(next);
    } catch (reason) {
      setError(String(reason));
    }
  }, [sessionId]);

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
    [busy, refreshStatus],
  );

  const startBrowser = useCallback(async () => {
    if (!sessionId) return;
    await runBrowserAction(async () => {
      const next = projectPath
        ? await invoke<BrowserStatus>("browser_start", {
            sessionId,
            projectPath,
          })
        : await invoke<BrowserStatus>("browser_start_plain_chat", { sessionId });
      setStatus(next);
    }, { refresh: false });
  }, [projectPath, runBrowserAction, sessionId]);

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
      setLogs((prev) => [
        ...prev.slice(-30),
        `已导入 Chrome Profile：${result.profileName} → ${result.targetPath}`,
      ]);
      const next = projectPath
        ? await invoke<BrowserStatus>("browser_start", {
            sessionId,
            projectPath,
          })
        : await invoke<BrowserStatus>("browser_start_plain_chat", { sessionId });
      setStatus(next);
    }, { refresh: false });
  }, [busy, projectPath, runBrowserAction, sessionId]);

  const openCurrentUrl = useCallback(async () => {
    const url = status?.url?.trim();
    if (!url || url === "about:blank") return;
    try {
      await openUrl(url);
    } catch (reason) {
      setError(String(reason));
    }
  }, [status?.url]);

  const [urlInput, setUrlInput] = useState("");
  const [isEditingUrl, setIsEditingUrl] = useState(false);
  const urlInputRef = useRef<HTMLInputElement>(null);

  // Sync urlInput from status when not editing
  useEffect(() => {
    if (!isEditingUrl) {
      setUrlInput(status?.url || "about:blank");
    }
  }, [isEditingUrl, status?.url]);

  const navigateToUrl = useCallback(async () => {
    if (!sessionId || submittingUrlRef.current) return;
    let url = urlInput.trim();
    if (!url || url === "about:blank") {
      setIsEditingUrl(false);
      return;
    }
    // Auto-prepend https:// if missing
    if (!url.startsWith("http://") && !url.startsWith("https://")) {
      url = "https://" + url;
      setUrlInput(url);
    }
    setIsEditingUrl(false);
    submittingUrlRef.current = true;
    await runBrowserAction(async () => {
      await invoke("browser_navigate", {
        sessionId,
        url,
        projectPath: projectPath || null,
      });
    });
    window.setTimeout(() => {
      submittingUrlRef.current = false;
    }, 0);
  }, [projectPath, runBrowserAction, sessionId, urlInput]);

  const reloadPage = useCallback(async () => {
    if (!sessionId) return;
    await runBrowserAction(async () => {
      await invoke("browser_reload", {
        sessionId,
        projectPath: projectPath || null,
      });
    });
  }, [projectPath, runBrowserAction, sessionId]);

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

  useEffect(() => {
    if (!active || !sessionId) return;
    refreshStatus().catch(console.error);
  }, [active, refreshStatus, sessionId]);

  useEffect(() => {
    if (!sessionId) return;
    const unsubs = [
      listen<BrowserStatus>("browser-status", (event) => {
        if (event.payload.sessionId === sessionId) {
          setStatus(event.payload);
          setLogs((prev) =>
            event.payload.message
              ? [...prev.slice(-30), event.payload.message]
              : prev,
          );
        }
      }),
      listen<BrowserFrameEvent>("browser-frame", (event) => {
        if (event.payload.sessionId === sessionId) {
          drawFrame(event.payload);
        }
      }),
      listen<BrowserLogEvent>("browser-log", (event) => {
        if (event.payload.sessionId === sessionId) {
          setLogs((prev) => [...prev.slice(-30), event.payload.message]);
        }
      }),
    ];

    return () => {
      unsubs.forEach((unsub) => unsub.then((fn) => fn()).catch(() => {}));
    };
  }, [drawFrame, sessionId]);

  const state = status?.state ?? "closed";
  const connected = state !== "closed" && state !== "page_closed";
  const pageClosed = state === "page_closed";
  const statusText = sessionId
    ? status?.message
      ? `${state} · ${status.message}`
      : state
    : "未选择会话";
  const canOpenCurrentUrl = Boolean(status?.url && status.url !== "about:blank");

  return (
    <aside
      className="ai-browser-panel ai-migrated-browser-panel"
      style={{ width }}
    >
      <div className="ai-browser-header">
        <div className="ai-browser-title-block">
          <div className="ai-browser-title">
            CloakBrowser
          </div>
          <div
            className={connected ? "ai-browser-status is-connected" : "ai-browser-status"}
          >
            {statusText}
          </div>
        </div>
        <div className="ai-browser-actions">
          <button
            type="button"
            title="返回上一页"
            onClick={goBack}
            disabled={!sessionId || !connected || busy}
            className="ai-browser-icon-button"
          >
            <ArrowLeft size={14} />
          </button>
          <button
            type="button"
            title="启动浏览器"
            onClick={startBrowser}
            disabled={!sessionId || busy}
            className="ai-browser-icon-button"
          >
            <Power size={14} />
          </button>
          {status?.hasHeadedWindow && !status?.minimized ? (
            <button
              type="button"
              title="最小化窗口"
              onClick={minimizeBrowser}
              disabled={!sessionId || busy || !connected}
              className="ai-browser-icon-button"
            >
              <MonitorDown size={14} />
            </button>
          ) : (
            <button
              type="button"
              title={status?.hasHeadedWindow ? "恢复窗口" : "打开独立窗口"}
              onClick={reopenBrowser}
              disabled={!sessionId || busy || !connected}
              className="ai-browser-icon-button"
            >
              <Monitor size={14} />
            </button>
          )}
          <button
            type="button"
            title="关闭浏览器"
            onClick={stopBrowser}
            disabled={!sessionId || busy}
            className="ai-browser-icon-button"
          >
            <Square size={14} />
          </button>
          <button
            type="button"
            title="导入 Chrome 登录态"
            onClick={importChromeProfile}
            disabled={!sessionId || busy}
            className="ai-browser-icon-button"
          >
            <KeyRound size={14} />
          </button>
          <button
            type="button"
            title="外部浏览器打开"
            onClick={openCurrentUrl}
            disabled={!canOpenCurrentUrl}
            className="ai-browser-icon-button"
          >
            <ExternalLink size={14} />
          </button>
          {onToggleExpanded && (
            <button
              type="button"
              title={expanded ? "还原宽度" : "展开面板"}
              onClick={onToggleExpanded}
              className="ai-browser-icon-button"
            >
              {expanded ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
            </button>
          )}
          {onClose && (
            <button type="button" title="关闭面板" onClick={onClose} className="ai-browser-icon-button">
              <X size={14} />
            </button>
          )}
        </div>
      </div>

      <div className="ai-browser-urlbar">
        <button
          type="button"
          title="刷新页面"
          onClick={reloadPage}
          disabled={!sessionId || !connected || busy}
          className="ai-browser-icon-button"
        >
          <RefreshCw size={13} />
        </button>
        <div
          className={isEditingUrl ? "ai-browser-address is-editing" : "ai-browser-address"}
        >
          {connected && status?.url && status.url !== "about:blank" && status.url.startsWith("https://") && (
            <span className="ai-browser-lock">
              {"https"}
            </span>
          )}
          {isEditingUrl ? (
            <input
              ref={urlInputRef}
              type="text"
              value={urlInput}
              onChange={(e) => setUrlInput(e.target.value)}
              onKeyDown={(e: KeyboardEvent<HTMLInputElement>) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  navigateToUrl();
                } else if (e.key === "Escape") {
                  setIsEditingUrl(false);
                  setUrlInput(status?.url || "about:blank");
                }
              }}
              onBlur={() => {
                // Navigate on blur if URL changed
                if (urlInput.trim() && urlInput.trim() !== (status?.url || "about:blank")) {
                  navigateToUrl();
                } else {
                  setIsEditingUrl(false);
                }
              }}
              autoFocus
              className="ai-browser-address-input"
            />
          ) : (
            <span
              onClick={() => {
                setIsEditingUrl(true);
                setUrlInput(status?.url || "");
                setTimeout(() => urlInputRef.current?.select(), 0);
              }}
              className={status?.url && status.url !== "about:blank" ? "ai-browser-address-text" : "ai-browser-address-text is-empty"}
              title={status?.url ?? ""}
            >
              {status?.url || "about:blank"}
            </span>
          )}
        </div>
      </div>

      <div className="ai-browser-stage">
        {sessionId && connected ? (
          <>
            <canvas
              ref={canvasRef}
              onClick={handleCanvasClick}
              className={busy ? "ai-browser-canvas is-busy" : "ai-browser-canvas"}
            />
            {status?.minimized && (
              <div className="ai-browser-minimized">
                窗口已最小化 · 可在面板中点击操作
              </div>
            )}
          </>
        ) : (
          <div className="ai-browser-empty">
            {sessionId ? (
              <>
                {pageClosed && (
                  <div className="ai-browser-empty-block">
                    <div className="ai-browser-empty-title">
                      浏览器窗口已关闭
                    </div>
                    <button
                      type="button"
                      title="重新打开窗口"
                      onClick={reopenBrowser}
                      disabled={!sessionId || busy}
                      className="ai-browser-reopen-button"
                    >
                      <MonitorUp size={14} />
                      重新打开
                    </button>
                  </div>
                )}
                {!connected && !pageClosed && (
                  <div className="ai-browser-empty-copy">
                    点击上方 ⚡ 按钮启动浏览器
                  </div>
                )}
              </>
            ) : (
              <div className="ai-browser-empty-copy">选择一个会话后可启动浏览器</div>
            )}
          </div>
        )}
      </div>

      {(error || logs.length > 0) && (
        <div className="ai-browser-log chat-scroll">
          {error && <div className="ai-browser-log-error">{error}</div>}
          {logs.map((item, index) => (
            <div key={`${index}-${item}`}>{item}</div>
          ))}
        </div>
      )}
    </aside>
  );
}

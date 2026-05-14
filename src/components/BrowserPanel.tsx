import { useCallback, useEffect, useRef, useState } from "react";
import type { CSSProperties, MouseEvent } from "react";
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
}

export function BrowserPanel({
  sessionId,
  projectPath,
  width,
  active,
  expanded = false,
  onToggleExpanded,
  onClose,
}: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [status, setStatus] = useState<BrowserStatus | null>(null);
  const [logs, setLogs] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const drawFrame = useCallback((frame: BrowserFrameEvent) => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const image = new Image();
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

  const startBrowser = useCallback(async () => {
    if (!sessionId) return;
    setBusy(true);
    setError(null);
    try {
      const next = projectPath
        ? await invoke<BrowserStatus>("browser_start", {
            sessionId,
            projectPath,
          })
        : await invoke<BrowserStatus>("browser_start_plain_chat", { sessionId });
      setStatus(next);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }, [projectPath, sessionId]);

  const stopBrowser = useCallback(async () => {
    if (!sessionId) return;
    setBusy(true);
    setError(null);
    try {
      await invoke("browser_stop", { sessionId });
      await refreshStatus();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }, [refreshStatus, sessionId]);

  const goBack = useCallback(async () => {
    if (!sessionId || busy) return;
    setBusy(true);
    setError(null);
    try {
      await invoke("browser_go_back", {
        sessionId,
        projectPath: projectPath || null,
      });
      await refreshStatus();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }, [busy, projectPath, refreshStatus, sessionId]);

  const importChromeProfile = useCallback(async () => {
    if (!sessionId || busy) return;

    let candidates: BrowserProfileCandidate[] = [];
    let scanMessage = "";
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

    setBusy(true);
    setError(null);
    try {
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
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }, [busy, projectPath, sessionId]);

  const openCurrentUrl = useCallback(async () => {
    const url = status?.url?.trim();
    if (!url || url === "about:blank") return;
    try {
      await openUrl(url);
    } catch (reason) {
      setError(String(reason));
    }
  }, [status?.url]);

  const handleCanvasClick = useCallback(
    async (event: MouseEvent<HTMLCanvasElement>) => {
      if (!sessionId || busy) return;
      const canvas = event.currentTarget;
      const rect = canvas.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0 || canvas.width <= 0 || canvas.height <= 0) {
        return;
      }

      const x = ((event.clientX - rect.left) / rect.width) * canvas.width;
      const y = ((event.clientY - rect.top) / rect.height) * canvas.height;

      setBusy(true);
      setError(null);
      try {
        await invoke("browser_click_at", {
          sessionId,
          projectPath: projectPath || null,
          x,
          y,
        });
      } catch (reason) {
        setError(String(reason));
      } finally {
        setBusy(false);
      }
    },
    [busy, projectPath, sessionId],
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
      unsubs.forEach((unsub) => unsub.then((fn) => fn()));
    };
  }, [drawFrame, sessionId]);

  const state = status?.state ?? "closed";
  const connected = state !== "closed";
  const statusText = sessionId
    ? status?.message
      ? `${state} · ${status.message}`
      : state
    : "未选择会话";
  const canOpenCurrentUrl = Boolean(status?.url && status.url !== "about:blank");

  return (
    <aside
      style={{
        width,
        height: "100%",
        display: "flex",
        flexDirection: "column",
        background: "var(--bg-sidebar)",
        borderLeft: "1px solid var(--border-dim)",
        overflow: "hidden",
      }}
    >
      <div
        style={{
          height: 42,
          flexShrink: 0,
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: 8,
          padding: "0 10px",
          borderBottom: "1px solid var(--border-dim)",
        }}
      >
        <div style={{ minWidth: 0, flex: 1 }}>
          <div style={{ fontSize: 12.5, fontWeight: 700, color: "var(--text-primary)" }}>
            CloakBrowser
          </div>
          <div
            style={{
              fontSize: 11,
              color: connected ? "var(--accent)" : "var(--text-hint)",
              whiteSpace: "nowrap",
              overflow: "hidden",
              textOverflow: "ellipsis",
              maxWidth: Math.max(120, width - 220),
            }}
          >
            {statusText}
          </div>
        </div>
        <div style={{ display: "flex", gap: 4, flexShrink: 0 }}>
          <button
            type="button"
            title="返回上一页"
            onClick={goBack}
            disabled={!sessionId || !connected || busy}
            style={iconButton}
          >
            <ArrowLeft size={14} />
          </button>
          <button type="button" title="刷新状态" onClick={refreshStatus} style={iconButton}>
            <RefreshCw size={14} />
          </button>
          <button
            type="button"
            title="启动浏览器"
            onClick={startBrowser}
            disabled={!sessionId || busy}
            style={iconButton}
          >
            <Power size={14} />
          </button>
          <button
            type="button"
            title="关闭浏览器"
            onClick={stopBrowser}
            disabled={!sessionId || busy}
            style={iconButton}
          >
            <Square size={14} />
          </button>
          <button
            type="button"
            title="导入 Chrome 登录态"
            onClick={importChromeProfile}
            disabled={!sessionId || busy}
            style={iconButton}
          >
            <KeyRound size={14} />
          </button>
          <button
            type="button"
            title="外部浏览器打开"
            onClick={openCurrentUrl}
            disabled={!canOpenCurrentUrl}
            style={iconButton}
          >
            <ExternalLink size={14} />
          </button>
          {onToggleExpanded && (
            <button
              type="button"
              title={expanded ? "还原宽度" : "展开面板"}
              onClick={onToggleExpanded}
              style={iconButton}
            >
              {expanded ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
            </button>
          )}
          {onClose && (
            <button type="button" title="关闭面板" onClick={onClose} style={iconButton}>
              <X size={14} />
            </button>
          )}
        </div>
      </div>

      <div
        style={{
          padding: 8,
          borderBottom: "1px solid var(--border-dim)",
          fontSize: 11,
          color: "var(--text-hint)",
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
        }}
        title={status?.url ?? ""}
      >
        {status?.url || "about:blank"}
      </div>

      <div
        style={{
          flex: 1,
          minHeight: 0,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          background: "var(--bg-panel)",
          overflow: "auto",
        }}
      >
        {sessionId ? (
          <canvas
            ref={canvasRef}
            onClick={handleCanvasClick}
            style={{
              width: "100%",
              height: "auto",
              display: "block",
              background: "var(--bg-card)",
              cursor: busy ? "progress" : "pointer",
            }}
          />
        ) : (
          <div style={{ color: "var(--text-hint)", fontSize: 12 }}>选择一个会话后可启动浏览器</div>
        )}
      </div>

      {(error || logs.length > 0) && (
        <div
          style={{
            maxHeight: 140,
            overflow: "auto",
            borderTop: "1px solid var(--border-dim)",
            padding: 8,
            fontSize: 11,
            lineHeight: 1.45,
            color: "var(--text-secondary)",
            fontFamily: "var(--font-mono)",
          }}
        >
          {error && <div style={{ color: "var(--danger)" }}>{error}</div>}
          {logs.map((item, index) => (
            <div key={`${index}-${item}`}>{item}</div>
          ))}
        </div>
      )}
    </aside>
  );
}

const iconButton: CSSProperties = {
  width: 26,
  height: 26,
  border: "1px solid var(--border-dim)",
  borderRadius: 6,
  background: "var(--bg-card)",
  color: "var(--text-secondary)",
  display: "inline-flex",
  alignItems: "center",
  justifyContent: "center",
  cursor: "pointer",
};

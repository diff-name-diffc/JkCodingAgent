import type React from "react";
import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { attachSmartCopy } from "./terminalCopyHelper";
import {
  DARK_THEME,
  LIGHT_THEME,
  initTerminal,
  loadWebglAddon,
  safeFit,
  createSmartWriter,
  createInputBatcher,
  createResizeScheduler,
} from "./terminalShared";
import { useIsDarkTheme } from "../hooks/useIsDarkTheme";
import { X } from "lucide-react";
import "@xterm/xterm/css/xterm.css";

interface ShellOutputEvent {
  shell_id: string;
  data: string;
}

interface Props {
  projectPath: string;
  projectId: string;
  isActive?: boolean;
  onClose: () => void;
  height?: number;
  onResizeStart?: (e: React.MouseEvent) => void;
}

const DRAIN_FRAME_BUDGET = 128 * 1024;

export function ShellTerminalPanel({
  projectPath,
  projectId,
  isActive = true,
  onClose,
  height = 240,
  onResizeStart,
}: Props) {
  const shellId = `shell:${projectId}`;
  const isDark = useIsDarkTheme();
  const containerRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const inputBatcherRef = useRef<ReturnType<typeof createInputBatcher> | null>(null);

  useEffect(() => {
    if (!containerRef.current) return;
    const container = containerRef.current;

    const { term, fitAddon } = initTerminal(5000);
    terminalRef.current = term;
    fitAddonRef.current = fitAddon;
    term.open(container);
    loadWebglAddon(term);
    const writer = createSmartWriter(term);
    const inputBatcher = createInputBatcher((data) => {
      invoke("send_input", { taskId: shellId, data }).catch(() => {});
    });
    inputBatcherRef.current = inputBatcher;

    const fit = () => {
      const s = safeFit(fitAddon, term);
      if (s)
        invoke("resize_pty", { taskId: shellId, cols: s.cols, rows: s.rows }).catch(() => {});
    };
    const resizeScheduler = createResizeScheduler(fit);

    setTimeout(() => {
      resizeScheduler.flush();
      invoke<void>("open_shell", {
        shellId,
        projectPath,
        cols: term.cols,
        rows: term.rows,
      }).catch(console.error);
      term.focus();
    }, 50);

    const disposeSmartCopy = attachSmartCopy(term);
    const disposeOnData = term.onData((data) => {
      inputBatcher.push(data);
    });

    const resizeObserver = new ResizeObserver(() => {
      resizeScheduler.schedule();
    });
    resizeObserver.observe(container);

    const handleVisibilityChange = () => {
      if (document.visibilityState !== "visible" || !terminalRef.current) return;
      window.requestAnimationFrame(() => {
        resizeScheduler.flush();
        const t = terminalRef.current;
        if (t) {
          t.refresh(0, t.rows - 1);
          t.focus();
        }
      });
    };
    document.addEventListener("visibilitychange", handleVisibilityChange);

    let unlisten: (() => void) | null = null;
    let cleaned = false;
    const pendingOutputs: string[] = [];
    let pendingHead = 0;
    let rafId = 0;

    const compactPendingOutputs = () => {
      if (pendingHead === 0) return;
      if (pendingHead < 64 && pendingHead * 2 < pendingOutputs.length) return;
      pendingOutputs.splice(0, pendingHead);
      pendingHead = 0;
    };

    const drainPendingOutputs = () => {
      rafId = 0;
      let bytesThisFrame = 0;
      const chunks: string[] = [];
      while (pendingHead < pendingOutputs.length && bytesThisFrame < DRAIN_FRAME_BUDGET) {
        const chunk = pendingOutputs[pendingHead++];
        chunks.push(chunk);
        bytesThisFrame += chunk.length;
      }
      compactPendingOutputs();
      if (chunks.length > 0) {
        writer.write(chunks.length === 1 ? chunks[0] : chunks.join(""));
      }
      if (pendingHead < pendingOutputs.length) {
        rafId = requestAnimationFrame(drainPendingOutputs);
      }
    };

    listen<ShellOutputEvent>("shell-output", (event) => {
      if (event.payload.shell_id === shellId && terminalRef.current) {
        pendingOutputs.push(event.payload.data);
        if (!rafId) {
          rafId = requestAnimationFrame(drainPendingOutputs);
        }
      }
    }).then((fn) => {
      if (cleaned) {
        fn(); // already unmounted, unlisten immediately
      } else {
        unlisten = fn;
      }
    });

    return () => {
      cleaned = true;
      unlisten?.();
      disposeSmartCopy();
      inputBatcher.dispose();
      inputBatcherRef.current = null;
      disposeOnData.dispose();
      if (rafId) cancelAnimationFrame(rafId);
      resizeScheduler.dispose();
      resizeObserver.disconnect();
      document.removeEventListener("visibilitychange", handleVisibilityChange);
      terminalRef.current = null;
      fitAddonRef.current = null;
      term.dispose();
      invoke("kill_shell", { shellId }).catch(() => {});
    };
  }, [shellId, projectPath]);

  useEffect(() => {
    if (!isActive) return;
    window.requestAnimationFrame(() => {
      if (!fitAddonRef.current || !terminalRef.current) return;
      const s = safeFit(fitAddonRef.current, terminalRef.current);
      if (s)
        invoke("resize_pty", { taskId: shellId, cols: s.cols, rows: s.rows }).catch(() => {});
      terminalRef.current.refresh(0, terminalRef.current.rows - 1);
      terminalRef.current.focus();
    });
  }, [isActive, shellId]);

  useEffect(() => {
    const term = terminalRef.current;
    if (!term) return;
    term.options.theme = isDark ? DARK_THEME : LIGHT_THEME;
    term.refresh(0, term.rows - 1);
  }, [isDark]);

  return (
    <div
      className="ai-shell-terminal-panel ai-migrated-shell-terminal"
      style={{ height, background: isDark ? DARK_THEME.background : LIGHT_THEME.background }}
    >
      {/* Drag handle */}
      {onResizeStart && (
        <div
          onMouseDown={onResizeStart}
          className="ai-shell-terminal-resize"
        />
      )}
      {/* Header */}
      <div className="ai-shell-terminal-header">
        <span className="ai-shell-terminal-title">
          终端
        </span>
        <button
          onClick={onClose}
          title="关闭终端"
          className="ai-shell-terminal-close"
        >
          <X size={14} />
        </button>
      </div>
      {/* Terminal */}
      <div
        ref={containerRef}
        className="ai-shell-terminal-canvas"
      />
    </div>
  );
}

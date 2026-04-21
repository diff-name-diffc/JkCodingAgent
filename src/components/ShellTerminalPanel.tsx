import type React from "react";
import { useEffect, useRef, forwardRef, useImperativeHandle } from "react";
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
import { X } from "lucide-react";
import "@xterm/xterm/css/xterm.css";

interface ShellOutputEvent {
  shell_id: string;
  data: string;
}

export interface ShellTerminalPanelHandle {
  sendCommand: (cmd: string) => void;
}

interface Props {
  projectPath: string;
  projectId: string;
  isActive?: boolean;
  onClose: () => void;
  isDark: boolean;
  onReady?: () => void;
  height?: number;
  onResizeStart?: (e: React.MouseEvent) => void;
}

const DRAIN_FRAME_BUDGET = 128 * 1024;

export const ShellTerminalPanel = forwardRef<ShellTerminalPanelHandle, Props>(
  function ShellTerminalPanel(
    {
      projectPath,
      projectId,
      isActive = true,
      onClose,
      isDark,
      onReady,
      height = 240,
      onResizeStart,
    },
    ref,
  ) {
    const shellId = `shell:${projectId}`;
    const containerRef = useRef<HTMLDivElement>(null);
    const terminalRef = useRef<Terminal | null>(null);
    const fitAddonRef = useRef<FitAddon | null>(null);
    const inputBatcherRef = useRef<ReturnType<typeof createInputBatcher> | null>(null);
    const isDarkRef = useRef(isDark);
    const onReadyRef = useRef(onReady);
    isDarkRef.current = isDark;
    onReadyRef.current = onReady;

    useImperativeHandle(ref, () => ({
      sendCommand: (cmd: string) => {
        const batcher = inputBatcherRef.current;
        if (batcher) {
          batcher.push(cmd);
          batcher.flush();
          return;
        }
        invoke("send_input", { taskId: shellId, data: cmd }).catch(console.error);
      },
    }));

    useEffect(() => {
      if (!containerRef.current) return;
      const container = containerRef.current;

      const { term, fitAddon } = initTerminal(isDarkRef.current, 5000);
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
        })
          .then(() => {
            setTimeout(() => onReadyRef.current?.(), 300);
          })
          .catch(console.error);
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
      if (terminalRef.current) {
        terminalRef.current.options.theme = isDark ? DARK_THEME : LIGHT_THEME;
      }
    }, [isDark]);

    return (
      <div
        style={{
          flexShrink: 0,
          height,
          borderTop: "1px solid var(--border-dim)",
          display: "flex",
          flexDirection: "column",
          background: isDark ? DARK_THEME.background : LIGHT_THEME.background,
        }}
      >
        {/* Drag handle */}
        {onResizeStart && (
          <div
            onMouseDown={onResizeStart}
            style={{
              height: 4,
              flexShrink: 0,
              cursor: "row-resize",
              background: "transparent",
            }}
          />
        )}
        {/* Header */}
        <div
          style={{
            height: 32,
            flexShrink: 0,
            display: "flex",
            alignItems: "center",
            padding: "0 10px 0 14px",
            borderBottom: "1px solid var(--border-dim)",
            background: "var(--bg-sidebar)",
            gap: 8,
          }}
        >
          <span style={{ fontSize: 12, fontWeight: 600, color: "var(--text-primary)", flex: 1 }}>
            终端
          </span>
          <button
            onClick={onClose}
            title="关闭终端"
            style={{
              background: "none",
              border: "none",
              cursor: "pointer",
              padding: 3,
              borderRadius: 4,
              display: "flex",
              alignItems: "center",
              color: "var(--text-hint)",
            }}
          >
            <X size={14} />
          </button>
        </div>
        {/* Terminal */}
        <div
          ref={containerRef}
          style={{ flex: 1, overflow: "hidden", padding: "4px 6px", cursor: "text" }}
        />
      </div>
    );
  },
);

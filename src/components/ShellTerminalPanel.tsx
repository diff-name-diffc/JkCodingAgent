import type React from "react";
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
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
import { useSplitterKeyboard } from "../hooks/use-splitter-keyboard";
import { subscribeShellOutput } from "./shell-output-bus";
import { TERMINAL_HEIGHT_LIMITS, terminalDragBounds } from "./project/workspace-budget";
import { DEFAULT_WORKSPACE_PREFS } from "./project/workspace-prefs";
import { ChevronDown, X } from "lucide-react";
import "@xterm/xterm/css/xterm.css";

interface Props {
  projectPath: string;
  projectId: string;
  /** 所在项目工作区可见（项目切换保活时为 false）。 */
  isActive?: boolean;
  /** dock 显示（UI-19）：false = CSS 隐藏，组件保持挂载、PTY 保活。 */
  visible?: boolean;
  /** 隐藏面板（保留会话）：只收起 dock，不卸载组件、不杀 shell。 */
  onHide: () => void;
  /** 结束会话：卸载组件并 kill_shell；再次打开是全新 shell。 */
  onTerminate: () => void;
  height?: number;
  /** 键盘步进/双击复位的即时提交通道（UI-23c）；拖拽 mouseup 也走此通道提交。 */
  onResizeCommit?: (height: number) => void;
}

const DRAIN_FRAME_BUDGET = 128 * 1024;

export function ShellTerminalPanel({
  projectPath,
  projectId,
  isActive = true,
  visible = true,
  onHide,
  onTerminate,
  height = 240,
  onResizeCommit,
}: Props) {
  const shellId = `shell:${projectId}`;
  const isDark = useIsDarkTheme();
  /** 拖拽中的实时高度（UI-24 遗留⑥）：本地 state，重渲染限本面板子树；
   *  mouseup 才经 onResizeCommit 一次性写回 store，避免高频持久化 + 全树重渲染。 */
  const [dragHeight, setDragHeight] = useState<number | null>(null);
  const dragHeightRef = useRef(dragHeight);
  dragHeightRef.current = dragHeight;
  const renderedHeight = dragHeight ?? height;
  // 高度把手键盘化（UI-23c）：ArrowUp/Down 步进、Shift 大步、双击复位默认。
  const resizeKeyboard = useSplitterKeyboard({
    orientation: "horizontal",
    mode: "px",
    ariaLabel: "方向键调整终端高度，双击恢复默认",
    getValue: () => renderedHeight,
    getBounds: () => ({ min: TERMINAL_HEIGHT_LIMITS.min, max: TERMINAL_HEIGHT_LIMITS.max }),
    getDefaultValue: () => DEFAULT_WORKSPACE_PREFS.terminalHeight,
    onCommit: (next) => onResizeCommit?.(next),
  });
  const containerRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const inputBatcherRef = useRef<ReturnType<typeof createInputBatcher> | null>(null);
  // open_shell 的 50ms 延迟 id（UI-24 遗留⑧）：卸载时清掉，避免「先 kill_shell 后 open_shell」竞态。
  const openShellTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // 卸载兜底：拖拽中途 terminate/切项目时提交 + 摘监听（对齐 24a-4 dragCleanupRef 模式）。
  const dragCleanupRef = useRef<(() => void) | null>(null);
  useEffect(() => () => dragCleanupRef.current?.(), []);

  const handleResizeStart = (e: React.MouseEvent) => {
    e.preventDefault();
    const startY = e.clientY;
    const startHeight = renderedHeight;
    // 与 resolveWorkspaceBudget 同口径的视口钳制（拖拽期窗口尺寸不变，仅 mousedown 读一次）。
    const bounds = terminalDragBounds(window.innerHeight);
    const onMouseMove = (ev: MouseEvent) => {
      const next = Math.max(
        bounds.min,
        Math.min(bounds.max, startHeight + (startY - ev.clientY)),
      );
      setDragHeight(next);
    };
    const onMouseUp = () => {
      document.removeEventListener("mousemove", onMouseMove);
      document.removeEventListener("mouseup", onMouseUp);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      dragCleanupRef.current = null;
      const latest = dragHeightRef.current;
      setDragHeight(null);
      if (latest != null) onResizeCommit?.(latest);
    };
    dragCleanupRef.current = onMouseUp;
    document.body.style.cursor = "row-resize";
    document.body.style.userSelect = "none";
    document.addEventListener("mousemove", onMouseMove);
    document.addEventListener("mouseup", onMouseUp);
  };

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
      // 零尺寸守卫（UI-19）：dock 隐藏（display:none）期间 ResizeObserver
      // 会上报 0×0，此时不 fit、不 resize_pty；恢复显示时 RO 自然重触发。
      if (container.clientWidth === 0 || container.clientHeight === 0) return;
      const s = safeFit(fitAddon, term);
      if (s)
        invoke("resize_pty", { taskId: shellId, cols: s.cols, rows: s.rows }).catch(() => {});
    };
    const resizeScheduler = createResizeScheduler(fit);

    openShellTimerRef.current = setTimeout(() => {
      openShellTimerRef.current = null;
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

    // 单一全局监听者（UI-24 遗留⑧）：N 个保活终端共享一个 shell-output 订阅，
    // 由 shell-output-bus 按 shell_id 分发；退订是同步的。
    const unsubscribeShellOutput = subscribeShellOutput(shellId, (data) => {
      if (!terminalRef.current) return;
      pendingOutputs.push(data);
      if (!rafId) {
        rafId = requestAnimationFrame(drainPendingOutputs);
      }
    });

    return () => {
      if (openShellTimerRef.current) {
        clearTimeout(openShellTimerRef.current);
        openShellTimerRef.current = null;
      }
      unsubscribeShellOutput();
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
    // 挂载主 effect 只随 shell 身份重建：visible 隐藏/恢复不重跑（PTY 保活）。
  }, [shellId, projectPath]);

  // 重激活（项目切回 / dock 恢复显示）：重新 fit + resize + 刷新 + 聚焦。
  useEffect(() => {
    if (!isActive || !visible) return;
    window.requestAnimationFrame(() => {
      const box = containerRef.current;
      const fitAddon = fitAddonRef.current;
      const term = terminalRef.current;
      if (!box || box.clientWidth === 0 || box.clientHeight === 0 || !fitAddon || !term) return;
      const s = safeFit(fitAddon, term);
      if (s)
        invoke("resize_pty", { taskId: shellId, cols: s.cols, rows: s.rows }).catch(() => {});
      term.refresh(0, term.rows - 1);
      term.focus();
    });
  }, [isActive, visible, shellId]);

  useEffect(() => {
    const term = terminalRef.current;
    if (!term) return;
    term.options.theme = isDark ? DARK_THEME : LIGHT_THEME;
    term.refresh(0, term.rows - 1);
  }, [isDark]);

  return (
    <div
      className="ai-shell-terminal-panel"
      style={{
        height: renderedHeight,
        display: visible ? undefined : "none",
        background: isDark ? DARK_THEME.background : LIGHT_THEME.background,
      }}
    >
      {/* Drag handle（UI-23c：role=separator + 键盘步进 + 双击复位） */}
      {onResizeCommit && (
        <div
          {...resizeKeyboard}
          onMouseDown={handleResizeStart}
          className="ai-shell-terminal-resize"
        />
      )}
      {/* Header：隐藏（保留会话）与结束会话是两个语义（设计 §3.3）。 */}
      <div className="ai-shell-terminal-header">
        <span className="ai-shell-terminal-title">
          终端
        </span>
        <div className="ai-shell-terminal-actions">
          <button
            onClick={onHide}
            title="隐藏终端面板（保留会话，可从底部状态栏恢复）"
            aria-label="隐藏终端面板（保留会话）"
            className="ai-shell-terminal-hide"
          >
            <ChevronDown size={14} />
          </button>
          <button
            onClick={onTerminate}
            title="结束终端会话（终止 shell 进程）"
            aria-label="结束终端会话（终止 shell 进程）"
            className="ai-shell-terminal-close"
          >
            <X size={14} />
          </button>
        </div>
      </div>
      {/* Terminal */}
      <div
        ref={containerRef}
        className="ai-shell-terminal-canvas"
      />
    </div>
  );
}

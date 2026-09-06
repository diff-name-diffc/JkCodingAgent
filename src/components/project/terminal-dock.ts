/**
 * 终端 dock 的挂载/可见两态机（UI-19，纯函数）。
 *
 * 语义（设计 §3.3「隐藏 ≠ 结束进程」）：
 * - mounted：ShellTerminalPanel 组件挂载——xterm 实例、scrollback 与 PTY 存活；
 * - visible：dock 在主区显示（隐藏 = CSS display:none，不卸载组件）。
 *
 * 只有 terminate（面板头部「结束会话」）会卸载组件，走既有 cleanup 内的
 * kill_shell；之后再 open 经 open_shell 全新会话。hide/toggle 永远保活 PTY。
 */

export interface TerminalDockState {
  mounted: boolean;
  visible: boolean;
}

export type TerminalDockAction = "toggle" | "open" | "hide" | "terminate";

export const TERMINAL_DOCK_CLOSED: TerminalDockState = { mounted: false, visible: false };

export function nextTerminalDockState(
  state: TerminalDockState,
  action: TerminalDockAction,
): TerminalDockState {
  switch (action) {
    case "open":
      return { mounted: true, visible: true };
    case "hide":
      // 未挂载时 hide 保持关闭（不存在「隐藏但挂载」的空转态）。
      return state.mounted ? { mounted: true, visible: false } : TERMINAL_DOCK_CLOSED;
    case "terminate":
      return TERMINAL_DOCK_CLOSED;
    case "toggle":
      if (!state.mounted) return { mounted: true, visible: true };
      return { mounted: true, visible: !state.visible };
  }
}

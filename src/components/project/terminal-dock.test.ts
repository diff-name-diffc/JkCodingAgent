import { describe, expect, it } from "vitest";
import {
  TERMINAL_DOCK_CLOSED,
  nextTerminalDockState,
  type TerminalDockState,
} from "./terminal-dock";

const OPEN: TerminalDockState = { mounted: true, visible: true };
const HIDDEN: TerminalDockState = { mounted: true, visible: false };

describe("nextTerminalDockState", () => {
  it("关闭态 toggle = 首次打开（挂载 + 显示）", () => {
    expect(nextTerminalDockState(TERMINAL_DOCK_CLOSED, "toggle")).toEqual(OPEN);
  });

  it("open 幂等：已打开时再 open 状态不变", () => {
    expect(nextTerminalDockState(OPEN, "open")).toEqual(OPEN);
  });

  it("hide 只收起显示，组件保持挂载（PTY 保活）", () => {
    expect(nextTerminalDockState(OPEN, "hide")).toEqual(HIDDEN);
  });

  it("toggle 在显示/隐藏间往返，挂载不变", () => {
    expect(nextTerminalDockState(OPEN, "toggle")).toEqual(HIDDEN);
    expect(nextTerminalDockState(HIDDEN, "toggle")).toEqual(OPEN);
  });

  it("关闭态 hide 不产生「挂载但隐藏」的空转态", () => {
    expect(nextTerminalDockState(TERMINAL_DOCK_CLOSED, "hide")).toEqual(TERMINAL_DOCK_CLOSED);
  });

  it("terminate 卸载并关闭；之后再 open 是全新会话语义", () => {
    expect(nextTerminalDockState(OPEN, "terminate")).toEqual(TERMINAL_DOCK_CLOSED);
    expect(nextTerminalDockState(HIDDEN, "terminate")).toEqual(TERMINAL_DOCK_CLOSED);
    expect(nextTerminalDockState(TERMINAL_DOCK_CLOSED, "terminate")).toEqual(TERMINAL_DOCK_CLOSED);
    expect(nextTerminalDockState(TERMINAL_DOCK_CLOSED, "open")).toEqual(OPEN);
  });
});

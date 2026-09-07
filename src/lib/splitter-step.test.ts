import { describe, expect, it } from "vitest";
import {
  SPLITTER_PX_LARGE_STEP,
  SPLITTER_PX_STEP,
  SPLITTER_RATIO_LARGE_STEP,
  SPLITTER_RATIO_STEP,
  nextSplitterValue,
  resetSplitterValue,
  splitterKeyDelta,
} from "./splitter-step";

describe("splitterKeyDelta（方向键 → 增量映射）", () => {
  it("垂直分隔条：ArrowRight=+1 / ArrowLeft=-1", () => {
    expect(splitterKeyDelta("ArrowRight", "vertical")).toBe(1);
    expect(splitterKeyDelta("ArrowLeft", "vertical")).toBe(-1);
  });

  it("水平分隔条：ArrowUp=+1 / ArrowDown=-1", () => {
    expect(splitterKeyDelta("ArrowUp", "horizontal")).toBe(1);
    expect(splitterKeyDelta("ArrowDown", "horizontal")).toBe(-1);
  });

  it("invert 翻转映射（右缘测量的占比 / 右停靠面板）", () => {
    expect(splitterKeyDelta("ArrowLeft", "vertical", true)).toBe(1);
    expect(splitterKeyDelta("ArrowRight", "vertical", true)).toBe(-1);
    expect(splitterKeyDelta("ArrowDown", "horizontal", true)).toBe(1);
  });

  it("非方向键返回 null（含错轴方向键）", () => {
    expect(splitterKeyDelta("Enter", "vertical")).toBeNull();
    expect(splitterKeyDelta("ArrowUp", "vertical")).toBeNull();
    expect(splitterKeyDelta("ArrowLeft", "horizontal")).toBeNull();
    expect(splitterKeyDelta("a", "vertical")).toBeNull();
  });
});

describe("nextSplitterValue — ratio 型（会话↔编辑区 splitter 语义）", () => {
  const ratio = { mode: "ratio", min: 0, max: 1 } as const;

  it("小步 0.02：0.5 → 0.52 / 0.48", () => {
    expect(nextSplitterValue({ ...ratio, current: 0.5, delta: 1 })).toBeCloseTo(0.5 + SPLITTER_RATIO_STEP);
    expect(nextSplitterValue({ ...ratio, current: 0.5, delta: -1 })).toBeCloseTo(0.5 - SPLITTER_RATIO_STEP);
  });

  it("Shift 大步 0.1：0.5 → 0.6", () => {
    expect(nextSplitterValue({ ...ratio, current: 0.5, delta: 1, shift: true })).toBeCloseTo(
      0.5 + SPLITTER_RATIO_LARGE_STEP,
    );
  });

  it("钳制在 [0,1] 边界，边界上继续按键值不变", () => {
    expect(nextSplitterValue({ ...ratio, current: 0.99, delta: 1 })).toBe(1);
    expect(nextSplitterValue({ ...ratio, current: 1, delta: 1 })).toBe(1);
    expect(nextSplitterValue({ ...ratio, current: 0.01, delta: -1 })).toBe(0);
    expect(nextSplitterValue({ ...ratio, current: 0, delta: -1 })).toBe(0);
  });

  it("消除浮点误差累积（两位小数）", () => {
    let value = 0.5;
    for (let i = 0; i < 3; i += 1) {
      value = nextSplitterValue({ ...ratio, current: value, delta: 1 });
    }
    expect(value).toBe(0.56);
  });
});

describe("nextSplitterValue — px 型（导航/侧栏/终端/浏览器面板）", () => {
  const nav = { mode: "px", min: 216, max: 320 } as const;

  it("小步 8px / Shift 大步 48px", () => {
    expect(nextSplitterValue({ ...nav, current: 248, delta: 1 })).toBe(248 + SPLITTER_PX_STEP);
    expect(nextSplitterValue({ ...nav, current: 280, delta: -1, shift: true })).toBe(
      280 - SPLITTER_PX_LARGE_STEP,
    );
  });

  it("钳制在站点边界（ContextNav 216–320）", () => {
    expect(nextSplitterValue({ ...nav, current: 318, delta: 1 })).toBe(320);
    expect(nextSplitterValue({ ...nav, current: 320, delta: 1 })).toBe(320);
    expect(nextSplitterValue({ ...nav, current: 218, delta: -1 })).toBe(216);
    expect(nextSplitterValue({ ...nav, current: 216, delta: -1 })).toBe(216);
  });

  it("终端高度边界（TERMINAL_HEIGHT_LIMITS 100–600）", () => {
    const terminal = { mode: "px", min: 100, max: 600 } as const;
    expect(nextSplitterValue({ ...terminal, current: 240, delta: 1 })).toBe(248);
    expect(nextSplitterValue({ ...terminal, current: 596, delta: 1 })).toBe(600);
    expect(nextSplitterValue({ ...terminal, current: 104, delta: -1 })).toBe(100);
  });

  it("结果恒为有限整数（px）", () => {
    const value = nextSplitterValue({ ...nav, current: 248.4, delta: 1 });
    expect(Number.isInteger(value)).toBe(true);
  });
});

describe("resetSplitterValue（双击复位）", () => {
  it("复位到默认值", () => {
    expect(resetSplitterValue(248, 216, 320)).toBe(248);
    expect(resetSplitterValue(0.5, 0, 1)).toBe(0.5);
  });

  it("默认值越界时被钳制", () => {
    expect(resetSplitterValue(999, 216, 320)).toBe(320);
    expect(resetSplitterValue(-5, 0, 1)).toBe(0);
  });
});

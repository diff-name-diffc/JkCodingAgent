import { describe, expect, it } from "vitest";
import {
  OVERSCAN_ROWS,
  ROW_ESTIMATE_PX,
  WINDOWING_THRESHOLD,
  shouldUseWindowing,
} from "./message-list-metrics";

describe("shouldUseWindowing — 阈值边界（验收 300/301）", () => {
  it("300 条不开窗", () => {
    expect(shouldUseWindowing(WINDOWING_THRESHOLD)).toBe(false);
  });

  it("301 条开窗", () => {
    expect(shouldUseWindowing(WINDOWING_THRESHOLD + 1)).toBe(true);
  });

  it("0/1 条不开窗", () => {
    expect(shouldUseWindowing(0)).toBe(false);
    expect(shouldUseWindowing(1)).toBe(false);
  });

  it("1000 条开窗", () => {
    expect(shouldUseWindowing(1000)).toBe(true);
  });

  it("参数化阈值生效", () => {
    expect(shouldUseWindowing(10, 10)).toBe(false);
    expect(shouldUseWindowing(11, 10)).toBe(true);
  });
});

describe("窗口化常量 — virtualizer 参数单一出处", () => {
  it("估高/overscan 与 24a 语义一致（180px / 8 行）", () => {
    expect(ROW_ESTIMATE_PX).toBe(180);
    expect(OVERSCAN_ROWS).toBe(8);
    expect(WINDOWING_THRESHOLD).toBe(300);
  });
});

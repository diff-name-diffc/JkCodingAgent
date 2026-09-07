import { describe, expect, it } from "vitest";
import {
  OVERSCAN_ROWS,
  ROW_ESTIMATE_PX,
  WINDOWING_THRESHOLD,
  computeWindowRange,
  windowSpacerHeights,
} from "./message-list-metrics";

describe("computeWindowRange — 阈值边界（验收 300/301）", () => {
  it("300 条不开窗：全量渲染", () => {
    const range = computeWindowRange({
      itemCount: WINDOWING_THRESHOLD,
      scrollTop: 99999,
      viewportHeight: 720,
    });
    expect(range).toEqual({ useWindowing: false, startIndex: 0, endIndex: 300 });
  });

  it("301 条开窗", () => {
    const range = computeWindowRange({
      itemCount: WINDOWING_THRESHOLD + 1,
      scrollTop: 0,
      viewportHeight: 720,
    });
    expect(range.useWindowing).toBe(true);
    expect(range.startIndex).toBe(0);
    // ceil(720/180)=4 + overscan 8 = 12
    expect(range.endIndex).toBe(Math.ceil(720 / ROW_ESTIMATE_PX) + OVERSCAN_ROWS);
  });

  it("空列表与 1 条：不开窗，endIndex=itemCount", () => {
    expect(computeWindowRange({ itemCount: 0, scrollTop: 0, viewportHeight: 720 })).toEqual({
      useWindowing: false,
      startIndex: 0,
      endIndex: 0,
    });
    expect(computeWindowRange({ itemCount: 1, scrollTop: 0, viewportHeight: 0 })).toEqual({
      useWindowing: false,
      startIndex: 0,
      endIndex: 1,
    });
  });
});

describe("computeWindowRange — 开窗后的区间推算", () => {
  const base = { itemCount: 1000, viewportHeight: 720 };

  it("scrollTop=0：startIndex 钳 0，endIndex=视口行数+overscan", () => {
    const range = computeWindowRange({ ...base, scrollTop: 0 });
    expect(range.startIndex).toBe(0);
    expect(range.endIndex).toBe(12);
  });

  it("中段：按估高整除 ± overscan", () => {
    // scrollTop=18000 → floor(18000/180)=100 - 8 = 92；
    // ceil((18000+720)/180)=104 + 8 = 112
    const range = computeWindowRange({ ...base, scrollTop: 18000 });
    expect(range.startIndex).toBe(92);
    expect(range.endIndex).toBe(112);
  });

  it("底部：endIndex 钳到 itemCount", () => {
    const range = computeWindowRange({ ...base, scrollTop: 1000 * ROW_ESTIMATE_PX });
    expect(range.endIndex).toBe(1000);
    expect(range.startIndex).toBeLessThan(1000);
  });

  it("负值防御：scrollTop/viewportHeight 负数按 0 处理", () => {
    const range = computeWindowRange({ ...base, scrollTop: -500, viewportHeight: -1 });
    expect(range.startIndex).toBe(0);
    expect(range.endIndex).toBe(OVERSCAN_ROWS);
  });

  it("参数化：自定义估高/overscan/阈值生效", () => {
    const range = computeWindowRange({
      itemCount: 50,
      scrollTop: 1000,
      viewportHeight: 500,
      rowEstimate: 100,
      overscan: 2,
      windowingThreshold: 10,
    });
    // floor(1000/100)=10-2=8；ceil(1500/100)=15+2=17
    expect(range).toEqual({ useWindowing: true, startIndex: 8, endIndex: 17 });
  });
});

describe("windowSpacerHeights — 占位高度", () => {
  it("未开窗：上下占位为 0", () => {
    const range = computeWindowRange({ itemCount: 100, scrollTop: 0, viewportHeight: 720 });
    expect(windowSpacerHeights(range, 100)).toEqual({ top: 0, bottom: 0 });
  });

  it("开窗：上占位=startIndex×估高，下占位=(count-endIndex)×估高", () => {
    const range = computeWindowRange({ itemCount: 1000, scrollTop: 18000, viewportHeight: 720 });
    expect(windowSpacerHeights(range, 1000)).toEqual({
      top: range.startIndex * ROW_ESTIMATE_PX,
      bottom: (1000 - range.endIndex) * ROW_ESTIMATE_PX,
    });
  });

  it("下占位不产生负值（endIndex 钳制后）", () => {
    const range = computeWindowRange({ itemCount: 1000, scrollTop: 999999, viewportHeight: 720 });
    expect(windowSpacerHeights(range, 1000).bottom).toBe(0);
  });
});

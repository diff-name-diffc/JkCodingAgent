import { describe, expect, it } from "vitest";
import { isRovingKey, nextRovingIndex } from "./roving-index";

describe("nextRovingIndex", () => {
  it("四方向 ±1（both 轴向默认）", () => {
    expect(nextRovingIndex({ count: 4, current: 1, key: "ArrowRight" })).toBe(2);
    expect(nextRovingIndex({ count: 4, current: 1, key: "ArrowLeft" })).toBe(0);
    expect(nextRovingIndex({ count: 4, current: 1, key: "ArrowDown" })).toBe(2);
    expect(nextRovingIndex({ count: 4, current: 1, key: "ArrowUp" })).toBe(0);
  });

  it("orientation=horizontal 时 ↑↓ 不参与（原样返回）", () => {
    expect(nextRovingIndex({ count: 4, current: 1, key: "ArrowUp", orientation: "horizontal" })).toBe(1);
    expect(nextRovingIndex({ count: 4, current: 1, key: "ArrowRight", orientation: "horizontal" })).toBe(2);
  });

  it("orientation=vertical 时 ←→ 不参与", () => {
    expect(nextRovingIndex({ count: 4, current: 1, key: "ArrowLeft", orientation: "vertical" })).toBe(1);
    expect(nextRovingIndex({ count: 4, current: 1, key: "ArrowDown", orientation: "vertical" })).toBe(2);
  });

  it("wrap=true（默认）首尾环绕", () => {
    expect(nextRovingIndex({ count: 4, current: 3, key: "ArrowRight" })).toBe(0);
    expect(nextRovingIndex({ count: 4, current: 0, key: "ArrowLeft" })).toBe(3);
  });

  it("wrap=false 边界钳制", () => {
    expect(nextRovingIndex({ count: 4, current: 3, key: "ArrowRight", wrap: false })).toBe(3);
    expect(nextRovingIndex({ count: 4, current: 0, key: "ArrowLeft", wrap: false })).toBe(0);
  });

  it("Home/End 跳首尾", () => {
    expect(nextRovingIndex({ count: 4, current: 2, key: "Home" })).toBe(0);
    expect(nextRovingIndex({ count: 4, current: 2, key: "End" })).toBe(3);
  });

  it("count=0 恒 0（不产生 -1）；count=1 恒 0", () => {
    expect(nextRovingIndex({ count: 0, current: 0, key: "ArrowDown" })).toBe(0);
    expect(nextRovingIndex({ count: 0, current: -1, key: "ArrowUp" })).toBe(0);
    expect(nextRovingIndex({ count: 1, current: 0, key: "ArrowDown" })).toBe(0);
    expect(nextRovingIndex({ count: 1, current: 0, key: "End" })).toBe(0);
  });

  it("current=-1（越界）先钳制到 0 再按 wrap 语义计算", () => {
    expect(nextRovingIndex({ count: 4, current: -1, key: "ArrowDown" })).toBe(1);
    // 钳制后 ArrowUp 从 0 环绕到末项；「焦点在列表外」的入列方向语义由
    // lib/list-focus 特判（ArrowDown→首项 / ArrowUp→末项），不走本数学。
    expect(nextRovingIndex({ count: 4, current: -1, key: "ArrowUp" })).toBe(3);
  });

  it("非导航键原样返回钳制后的 current", () => {
    expect(nextRovingIndex({ count: 4, current: 2, key: "Enter" })).toBe(2);
    expect(nextRovingIndex({ count: 4, current: 9, key: "a" })).toBe(3);
  });
});

describe("isRovingKey", () => {
  it("方向键与 Home/End 为导航键", () => {
    for (const key of ["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End"]) {
      expect(isRovingKey(key)).toBe(true);
    }
  });

  it("字符键/功能键不是导航键", () => {
    expect(isRovingKey("Enter")).toBe(false);
    expect(isRovingKey("a")).toBe(false);
    expect(isRovingKey("Escape")).toBe(false);
  });

  it("轴向过滤：horizontal 只认 ←→/Home/End", () => {
    expect(isRovingKey("ArrowUp", "horizontal")).toBe(false);
    expect(isRovingKey("ArrowLeft", "horizontal")).toBe(true);
    expect(isRovingKey("ArrowDown", "vertical")).toBe(true);
    expect(isRovingKey("ArrowRight", "vertical")).toBe(false);
  });
});

import { describe, expect, it } from "vitest";
import { resolveDockedPanelDefaultWidth } from "./useDockedBrowserPanel";

/** UI-15a：停靠面板默认宽解析——比例路径保持历史行为，像素锚定路径供
 * 固定区间规格（架构助手 320–400px）使用，两者统一受 min/maxRatio 钳制。 */
describe("resolveDockedPanelDefaultWidth", () => {
  it("未传 defaultWidthPx 时沿用视口比例默认（浏览器面板历史行为）", () => {
    // 默认 metrics：min 420 / ratio 0.4 / maxRatio 0.75 → 1600 × 0.4 = 640
    expect(resolveDockedPanelDefaultWidth(1600, {})).toBe(640);
  });

  it("比例默认低于 minWidth 时钳到 minWidth", () => {
    // 800 × 0.4 = 320 < 420
    expect(resolveDockedPanelDefaultWidth(800, {})).toBe(420);
  });

  it("defaultWidthPx 像素锚定：大视口下不再随比例漂移", () => {
    expect(
      resolveDockedPanelDefaultWidth(1920, { minWidth: 320, defaultWidthPx: 360, maxRatio: 0.6 }),
    ).toBe(360);
  });

  it("defaultWidthPx 低于 minWidth 时钳到 minWidth", () => {
    expect(
      resolveDockedPanelDefaultWidth(1920, { minWidth: 320, defaultWidthPx: 200, maxRatio: 0.6 }),
    ).toBe(320);
  });

  it("defaultWidthPx 高于 maxRatio 上限时钳到面板上限（不低于 minWidth）", () => {
    // 上限 = max(320, floor(1000 × 0.25)) = 320
    expect(
      resolveDockedPanelDefaultWidth(1000, { minWidth: 320, defaultWidthPx: 800, maxRatio: 0.25 }),
    ).toBe(320);
    // 上限 = floor(2000 × 0.3) = 600
    expect(
      resolveDockedPanelDefaultWidth(2000, { minWidth: 320, defaultWidthPx: 800, maxRatio: 0.3 }),
    ).toBe(600);
  });
});

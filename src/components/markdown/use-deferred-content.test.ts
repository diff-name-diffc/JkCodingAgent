import { describe, expect, it } from "vitest";
import {
  LARGE_TEXT_DEFER_THRESHOLD,
  shouldDeferLargeRender,
  STREAM_THROTTLE_MS,
  throttleSchedule,
} from "./use-deferred-content";

describe("use-deferred-content 常量与纯函数", () => {
  it("流式节流节奏（150ms）", () => {
    expect(STREAM_THROTTLE_MS).toBe(150);
  });

  it("大文本阈值与 AGENTS.md 约定一致（10KB）", () => {
    expect(LARGE_TEXT_DEFER_THRESHOLD).toBe(10_000);
  });

  it("shouldDeferLargeRender 边界：恰好阈值不降级，超过才降级", () => {
    expect(shouldDeferLargeRender(LARGE_TEXT_DEFER_THRESHOLD)).toBe(false);
    expect(shouldDeferLargeRender(LARGE_TEXT_DEFER_THRESHOLD + 1)).toBe(true);
    expect(shouldDeferLargeRender(0)).toBe(false);
  });
});

describe("throttleSchedule（连续 token 流下不饿死）", () => {
  it("超过节流窗口立即冲刷", () => {
    expect(throttleSchedule(150)).toBe("immediate");
    expect(throttleSchedule(500)).toBe("immediate");
  });

  it("窗口内按剩余时间调度——触发时刻固定在上次冲刷 + 150ms", () => {
    // 若每次 content 变化都重新计 150ms，连续流式下定时不触发（画面冻结）；
    // 延迟必须是 150 - elapsed，保证冲刷点不被推迟。
    expect(throttleSchedule(0)).toBe(150);
    expect(throttleSchedule(100)).toBe(50);
    expect(throttleSchedule(149)).toBe(1);
  });
});

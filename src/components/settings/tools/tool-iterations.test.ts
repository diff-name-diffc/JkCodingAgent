import { describe, expect, it } from "vitest";
import type { AhaSettingsV2 } from "../../../types";
import {
  DEFAULT_TOOL_ITERATIONS,
  TOOL_ITERATIONS_RANGE,
  maxToolIterationsValue,
  patchMaxToolIterations,
} from "./tool-iterations";

function settingsWith(maxToolIterations?: number): AhaSettingsV2 {
  return { maxToolIterations } as AhaSettingsV2;
}

describe("patchMaxToolIterations", () => {
  it("设置与清除互为逆操作", () => {
    const base = settingsWith();
    const set = patchMaxToolIterations(base, 2000);
    expect(maxToolIterationsValue(set)).toBe(2000);
    const cleared = patchMaxToolIterations(set, undefined);
    expect(maxToolIterationsValue(cleared)).toBeUndefined();
    expect("maxToolIterations" in cleared).toBe(false);
  });

  it("不改动原对象（自动保存管线依赖不可变更新）", () => {
    const base = settingsWith(500);
    const next = patchMaxToolIterations(base, 800);
    expect(maxToolIterationsValue(base)).toBe(500);
    expect(maxToolIterationsValue(next)).toBe(800);
  });
});

describe("TOOL_ITERATIONS_RANGE", () => {
  it("区间与默认值钉死 agent/config.rs 常量字面值（双端同步守护）", () => {
    // 镜像值来自 src-tauri 的 agent/config.rs 常量组：
    // MAX_TOOL_ITERATIONS_RANGE=(1,10000)、DEFAULT_MAX_TOOL_ITERATIONS=1000。
    // 钉死字面值使任一侧漂移都会令本用例失败，强制开发者显式同步另一侧。
    expect(TOOL_ITERATIONS_RANGE).toEqual({ min: 1, max: 10000 });
    expect(DEFAULT_TOOL_ITERATIONS).toBe(1000);
    expect(DEFAULT_TOOL_ITERATIONS).toBeGreaterThanOrEqual(TOOL_ITERATIONS_RANGE.min);
    expect(DEFAULT_TOOL_ITERATIONS).toBeLessThanOrEqual(TOOL_ITERATIONS_RANGE.max);
  });
});

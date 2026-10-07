import { describe, expect, it } from "vitest";
import type { AhaSettingsV2 } from "../../../types";
import {
  FETCH_TIMEOUT_RANGE,
  IMAGE_TIMEOUT_RANGE,
  patchToolTimeouts,
  timeoutFieldValue,
  TOOL_TIMEOUT_FIELD_DEFS,
} from "./tool-timeouts";

function settingsWith(toolTimeouts?: AhaSettingsV2["toolTimeouts"]): AhaSettingsV2 {
  return { toolTimeouts } as AhaSettingsV2;
}

describe("patchToolTimeouts", () => {
  it("设置与清除字段互为逆操作", () => {
    const base = settingsWith();
    const set = patchToolTimeouts(base, "generateImageSecs", 180);
    expect(timeoutFieldValue(set, "generateImageSecs")).toBe(180);
    const cleared = patchToolTimeouts(set, "generateImageSecs", undefined);
    expect(timeoutFieldValue(cleared, "generateImageSecs")).toBeUndefined();
  });

  it("清除最后一个键后保留空对象（section 形态稳定）", () => {
    const only = settingsWith({ fetchImageSecs: 90 });
    const cleared = patchToolTimeouts(only, "fetchImageSecs", undefined);
    expect(cleared.toolTimeouts).toEqual({});
  });

  it("更新一个键不影响其它键", () => {
    const base = settingsWith({ generateImageSecs: 120, editImageSecs: 150 });
    const next = patchToolTimeouts(base, "editImageSecs", 200);
    expect(timeoutFieldValue(next, "generateImageSecs")).toBe(120);
    expect(timeoutFieldValue(next, "editImageSecs")).toBe(200);
  });
});

describe("TOOL_TIMEOUT_FIELD_DEFS", () => {
  it("键唯一、区间合法且默认值落在区间内", () => {
    const keys = TOOL_TIMEOUT_FIELD_DEFS.map((def) => def.key);
    expect(new Set(keys).size).toBe(keys.length);
    for (const def of TOOL_TIMEOUT_FIELD_DEFS) {
      expect(def.min).toBeGreaterThan(0);
      expect(def.max).toBeGreaterThan(def.min);
      expect(def.defaultSecs).toBeGreaterThanOrEqual(def.min);
      expect(def.defaultSecs).toBeLessThanOrEqual(def.max);
    }
  });

  it("区间与默认值钉死 spec.rs 常量组字面值（双端同步守护）", () => {
    // 镜像值来自 src-tauri 的 rig_ext/tools/spec.rs 常量组：
    // IMAGE_TOOL_CALL_TIMEOUT_RANGE=(30,300)、FETCH_IMAGE_CALL_TIMEOUT_RANGE=(10,300)、
    // IMAGE_TOOL_TIMEOUT_SECS=120（生成/编辑）、FETCH_IMAGE_TIMEOUT_SECS=60。
    // 钉死字面值使任一侧漂移都会令本用例失败，强制开发者显式同步另一侧。
    expect(IMAGE_TIMEOUT_RANGE).toEqual({ min: 30, max: 300 });
    expect(FETCH_TIMEOUT_RANGE).toEqual({ min: 10, max: 300 });
    expect(
      TOOL_TIMEOUT_FIELD_DEFS.map((def) => [def.key, def.min, def.max, def.defaultSecs]),
    ).toEqual([
      ["generateImageSecs", 30, 300, 120],
      ["editImageSecs", 30, 300, 120],
      ["fetchImageSecs", 10, 300, 60],
    ]);
  });
});

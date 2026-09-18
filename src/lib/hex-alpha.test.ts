import { describe, expect, it } from "vitest";
import { hexWithAlpha } from "./hex-alpha";

describe("hexWithAlpha", () => {
  it("给 6 位 hex 追加 alpha 后缀", () => {
    expect(hexWithAlpha("#297c70", "1f")).toBe("#297c701f");
    expect(hexWithAlpha("#FF8800", "AA")).toBe("#ff8800aa");
  });

  it("拒绝非 #rrggbb 颜色（返回 null 而非产出非法色值）", () => {
    expect(hexWithAlpha("var(--accent)", "1f")).toBeNull();
    expect(hexWithAlpha("#fff", "1f")).toBeNull();
    expect(hexWithAlpha("#297c7", "1f")).toBeNull();
    expect(hexWithAlpha("red", "1f")).toBeNull();
    expect(hexWithAlpha("", "1f")).toBeNull();
  });

  it("拒绝非法 alpha", () => {
    expect(hexWithAlpha("#297c70", "1")).toBeNull();
    expect(hexWithAlpha("#297c70", "")).toBeNull();
    expect(hexWithAlpha("#297c70", "1g")).toBeNull();
  });
});

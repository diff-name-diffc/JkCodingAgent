import { describe, expect, it } from "vitest";
import { resolveCanvasTheme } from "./architecture-theme";

describe("resolveCanvasTheme", () => {
  it("暗色主题映射为 dark", () => {
    expect(resolveCanvasTheme(true)).toBe("dark");
  });

  it("亮色主题映射为 light", () => {
    expect(resolveCanvasTheme(false)).toBe("light");
  });
});

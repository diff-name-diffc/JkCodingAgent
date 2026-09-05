import { describe, expect, it } from "vitest";
import { resolveTldrawColorScheme } from "./architecture-theme";

describe("resolveTldrawColorScheme", () => {
  it("暗色主题映射为 dark", () => {
    expect(resolveTldrawColorScheme(true)).toBe("dark");
  });

  it("亮色主题映射为 light", () => {
    expect(resolveTldrawColorScheme(false)).toBe("light");
  });
});

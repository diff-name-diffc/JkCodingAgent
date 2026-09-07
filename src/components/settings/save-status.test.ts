import { describe, expect, it } from "vitest";
import { deriveSaveStatus } from "./save-status";

describe("deriveSaveStatus", () => {
  it("loading 优先于其它状态", () => {
    expect(deriveSaveStatus({ loading: true, dirty: false, hasError: false })).toBe("loading");
    expect(deriveSaveStatus({ loading: true, dirty: true, hasError: true })).toBe("loading");
  });

  it("无错误、非 dirty 时为已保存（含初次加载完成未编辑）", () => {
    expect(deriveSaveStatus({ loading: false, dirty: false, hasError: false })).toBe("saved");
  });

  it("dirty（pending 或 saving）且无错误时为保存中", () => {
    expect(deriveSaveStatus({ loading: false, dirty: true, hasError: false })).toBe("saving");
  });

  it("有错误时为保存失败，优先于 saving", () => {
    expect(deriveSaveStatus({ loading: false, dirty: false, hasError: true })).toBe("error");
    // 失败后未再编辑：dirty=false，仍显示 error 而非 saved
    expect(deriveSaveStatus({ loading: false, dirty: true, hasError: true })).toBe("error");
  });
});

import { describe, expect, it } from "vitest";
import { formatAttachmentHint } from "./attachment-context";

describe("formatAttachmentHint", () => {
  it("两开关全关：无附带信息，返回 null", () => {
    expect(
      formatAttachmentHint({ attachScreenshot: false, attachSnapshot: false, shapeCount: 12 }),
    ).toBeNull();
  });

  it("两开 + 空画布：明确说明截图与快照都不会附带", () => {
    expect(
      formatAttachmentHint({ attachScreenshot: true, attachSnapshot: true, shapeCount: 0 }),
    ).toBe("画布为空：本次发送不附带截图与快照");
  });

  it("仅截图开 + 空画布：只提截图", () => {
    expect(
      formatAttachmentHint({ attachScreenshot: true, attachSnapshot: false, shapeCount: 0 }),
    ).toBe("画布为空：本次发送不附带截图");
  });

  it("仅快照开 + 空画布：只提快照", () => {
    expect(
      formatAttachmentHint({ attachScreenshot: false, attachSnapshot: true, shapeCount: 0 }),
    ).toBe("画布为空：本次发送不附带快照");
  });

  it("负数图形数按空画布处理", () => {
    expect(
      formatAttachmentHint({ attachScreenshot: true, attachSnapshot: true, shapeCount: -1 }),
    ).toBe("画布为空：本次发送不附带截图与快照");
  });

  it("仅截图开 + 有图形：只列截图并带数量", () => {
    expect(
      formatAttachmentHint({ attachScreenshot: true, attachSnapshot: false, shapeCount: 5 }),
    ).toBe("本次发送将附带：截图（5 个图形）");
  });

  it("仅快照开 + 有图形：只列快照", () => {
    expect(
      formatAttachmentHint({ attachScreenshot: false, attachSnapshot: true, shapeCount: 3 }),
    ).toBe("本次发送将附带：结构化快照");
  });

  it("两开 + 有图形：截图（N 个图形）· 结构化快照", () => {
    expect(
      formatAttachmentHint({ attachScreenshot: true, attachSnapshot: true, shapeCount: 12 }),
    ).toBe("本次发送将附带：截图（12 个图形） · 结构化快照");
  });
});

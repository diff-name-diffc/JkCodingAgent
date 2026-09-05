import { describe, expect, it } from "vitest";
import { canvasNotReadyReport, TLDR_LICENSE_GATE_SELECTOR } from "./canvas-block-info";

describe("canvasNotReadyReport", () => {
  it("无阻断信息时返回默认文案", () => {
    expect(canvasNotReadyReport(null)).toContain("画布未就绪");
    expect(canvasNotReadyReport(null)).toContain("程序未执行");
  });

  it("许可阻断附带 VITE_TLDRAW_LICENSE_KEY 修复指引", () => {
    const report = canvasNotReadyReport({ kind: "license" });
    expect(report).toContain("VITE_TLDRAW_LICENSE_KEY");
    expect(report).toContain("程序未执行");
  });

  it("崩溃阻断附带错误消息", () => {
    const report = canvasNotReadyReport({ kind: "crash", message: "boom" });
    expect(report).toContain("boom");
    expect(report).toContain("渲染崩溃");
  });

  it("意外关闭与视图未打开文案区分", () => {
    expect(canvasNotReadyReport({ kind: "unexpected" })).toContain("意外关闭");
  });

  it("门禁选择器指向 tldraw LicenseGate 占位节点", () => {
    expect(TLDR_LICENSE_GATE_SELECTOR).toBe('[data-testid="tl-license-expired"]');
  });
});

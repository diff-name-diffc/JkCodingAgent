import { describe, expect, it } from "vitest";
import { canvasNotReadyReport } from "./canvas-block-info";

describe("canvasNotReadyReport", () => {
  it("无阻断信息时返回默认文案", () => {
    expect(canvasNotReadyReport(null)).toContain("画布未就绪");
    expect(canvasNotReadyReport(null)).toContain("程序未执行");
  });

  it("崩溃阻断附带错误消息", () => {
    const report = canvasNotReadyReport({ kind: "crash", message: "boom" });
    expect(report).toContain("boom");
    expect(report).toContain("渲染崩溃");
  });
});

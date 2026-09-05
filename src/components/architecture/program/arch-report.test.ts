import { describe, expect, it } from "vitest";
import {
  ARCH_REPORT_MAX_CHARS,
  buildArchFailureReport,
  buildArchSuccessReport,
  truncateArchReport,
} from "./arch-report";

describe("report builders", () => {
  it("builds success report with ref mapping and screenshot reference", () => {
    const report = buildArchSuccessReport(
      {
        total: 3,
        created: 2,
        updated: 0,
        moved: 0,
        deleted: 0,
        arrows: 1,
        layouts: 0,
        reparented: 0,
        views: 0,
      },
      new Map([
        ["a1", "shape:Ab3xK9"],
        ["a2", "shape:Cd9yL2"],
      ]),
      12,
      "3f9a1b2c-0000-1111-2222-333344445555",
    );
    expect(report).toContain("执行成功：3 条指令");
    expect(report).toContain("a1→shape:Ab3xK9");
    expect(report).toContain("chat-image://3f9a1b2c-0000-1111-2222-333344445555");
    expect([...report].length).toBeLessThanOrEqual(ARCH_REPORT_MAX_CHARS);
  });

  it("reports reparent and view actions in the stats line", () => {
    const report = buildArchSuccessReport(
      {
        total: 3,
        created: 0,
        updated: 0,
        moved: 0,
        deleted: 0,
        arrows: 0,
        layouts: 0,
        reparented: 1,
        views: 2,
      },
      new Map(),
      8,
      null,
    );
    expect(report).toContain("容器 1");
    expect(report).toContain("视图 2");
  });

  it("keeps screenshot reference when truncating a huge report", () => {
    const hugeMap = new Map<string, string>();
    for (let i = 0; i < 60; i += 1) {
      hugeMap.set(`aliasNumber${i}`, `shape:xxxxxxxxxxxxxxxx${i}`);
    }
    const report = buildArchSuccessReport(
      {
        total: 40,
        created: 40,
        updated: 0,
        moved: 0,
        deleted: 0,
        arrows: 0,
        layouts: 0,
        reparented: 0,
        views: 0,
      },
      hugeMap,
      100,
      "image-id-1234567890abcdef",
    );
    expect([...report].length).toBeLessThanOrEqual(ARCH_REPORT_MAX_CHARS);
    expect(report).toContain("chat-image://image-id-1234567890abcdef");
  });

  it("builds failure report with rollback notice", () => {
    const report = buildArchFailureReport(2, "create_arrow", "from 引用的形状不存在：a9");
    expect(report).toContain("第 3 条指令（create_arrow）失败");
    expect(report).toContain("已整体回滚");
    expect(report).toContain("错误：");
  });

  it("truncateArchReport is a no-op for short reports", () => {
    expect(truncateArchReport("短报告")).toBe("短报告");
  });

  it("hard-truncates oversized text and preserves the trailing screenshot reference", () => {
    // 直接构造超长文本，真正走到硬截断分支（buildArchSuccessReport 会先把
    // 超长映射截到 8 条，报告远小于上限、覆盖不到该分支）。
    const longBody = "错".repeat(1200);
    const imageId = "shot-12345678";
    const truncated = truncateArchReport(`${longBody}\n执行区域截图：chat-image://${imageId}`);
    expect([...truncated].length).toBeLessThanOrEqual(ARCH_REPORT_MAX_CHARS);
    // 截断保留开头（结论/原因），截图引用尽力保留在末尾。
    expect(truncated.startsWith("错")).toBe(true);
    expect(truncated.endsWith(`chat-image://${imageId}`)).toBe(true);
    // 引用只出现一次且完整（正文为纯「错」填充，不含引用字样）。
    expect(truncated.split("chat-image://").length - 1).toBe(1);
  });

  it("hard-truncates oversized text without a screenshot reference", () => {
    const truncated = truncateArchReport("x".repeat(1200));
    expect([...truncated].length).toBeLessThanOrEqual(ARCH_REPORT_MAX_CHARS);
    expect(truncated.endsWith("…")).toBe(true);
  });
});

import { describe, expect, it } from "vitest";
import type { ToolActivityItem } from "./tool-activity";
import {
  categorizeTool,
  formatSummaryDuration,
  formatToolActivitySummary,
  summarizeToolActivity,
} from "./tool-activity-summary";

function item(partial: Partial<ToolActivityItem> & { id: string; name: string }): ToolActivityItem {
  return { status: "success", ...partial } as ToolActivityItem;
}

describe("categorizeTool", () => {
  const cases: Array<[string, string]> = [
    ["read_file", "read"],
    ["list_dir", "read"],
    ["write_file", "write"],
    ["edit_file", "write"],
    ["exec", "command"],
    ["local_zsh", "command"],
    ["ssh_exec", "command"],
    ["grep", "search"],
    ["glob", "search"],
    ["call_sub_agent", "subagent"],
    ["submit_graph", "graph"],
    ["run_tool_program", "program"],
    ["generate_image", "image"],
    ["edit_image", "image"],
    ["analyze_image", "image"],
    ["fetch_image", "image"],
    ["browser_navigate", "browser"],
    ["browser_click_at", "browser"],
    ["mcp__server__tool", "mcp"],
    ["notify_user_progress", "other"],
    ["some_future_tool", "other"],
  ];
  it.each(cases)("%s → %s", (name, expected) => {
    expect(categorizeTool(name)).toBe(expected);
  });
});

describe("summarizeToolActivity", () => {
  it("空数组 → total 0，文案为空串", () => {
    const summary = summarizeToolActivity([]);
    expect(summary).toEqual({
      total: 0,
      counts: {},
      failed: 0,
      running: 0,
      planned: 0,
      totalDurationMs: 0,
    });
    expect(formatToolActivitySummary(summary)).toBe("");
  });

  it("混合状态计数：failed/running/planned 互斥，planned 不计入 running", () => {
    const summary = summarizeToolActivity([
      item({ id: "1", name: "read_file", status: "success", durationMs: 100 }),
      item({ id: "2", name: "read_file", status: "success", durationMs: 200 }),
      item({ id: "3", name: "write_file", status: "success", durationMs: 300 }),
      item({ id: "4", name: "exec", status: "error", errorText: "错误：exit 1", durationMs: 400 }),
      item({ id: "5", name: "grep", status: "running" }),
      item({ id: "6", name: "glob", status: "running", planned: true }),
    ]);
    expect(summary.total).toBe(6);
    expect(summary.counts).toEqual({ read: 2, write: 1, command: 1, search: 2 });
    expect(summary.failed).toBe(1);
    expect(summary.running).toBe(1);
    expect(summary.planned).toBe(1);
    expect(summary.totalDurationMs).toBe(1000);
  });

  it("durationMs 缺失按 0 计", () => {
    const summary = summarizeToolActivity([
      item({ id: "1", name: "read_file" }),
      item({ id: "2", name: "exec", durationMs: 250 }),
    ]);
    expect(summary.totalDurationMs).toBe(250);
  });

  it("同输入两次调用输出深度相等（可复算）", () => {
    const items = [
      item({ id: "1", name: "read_file", durationMs: 10 }),
      item({ id: "2", name: "mcp__a__b", status: "error", errorText: "错误：x" }),
    ];
    expect(summarizeToolActivity(items)).toEqual(summarizeToolActivity(items));
    expect(formatToolActivitySummary(summarizeToolActivity(items))).toBe(
      formatToolActivitySummary(summarizeToolActivity(items)),
    );
  });
});

describe("formatToolActivitySummary", () => {
  it("类别按固定顺序拼接并附总耗时", () => {
    const summary = summarizeToolActivity([
      item({ id: "1", name: "exec", durationMs: 1500 }),
      item({ id: "2", name: "read_file", durationMs: 500 }),
      item({ id: "3", name: "read_file", durationMs: 500 }),
      item({ id: "4", name: "write_file", durationMs: 700 }),
    ]);
    expect(formatToolActivitySummary(summary)).toBe(
      "读取 2 个文件 · 修改 1 个文件 · 命令 1 · 总耗时 3.2s",
    );
  });

  it("全部未识别工具归入「其他」", () => {
    const summary = summarizeToolActivity([item({ id: "1", name: "notify_user_progress" })]);
    expect(formatToolActivitySummary(summary)).toBe("其他 1 · 总耗时 0ms");
  });
});

describe("formatSummaryDuration", () => {
  it("秒级以下显示 ms，以上显示一位小数秒", () => {
    expect(formatSummaryDuration(250)).toBe("250ms");
    expect(formatSummaryDuration(999)).toBe("999ms");
    expect(formatSummaryDuration(1000)).toBe("1.0s");
    expect(formatSummaryDuration(3200)).toBe("3.2s");
  });
});

import { describe, expect, it } from "vitest";
import type { ToolActivityItem } from "../dispatcher-chat/tool-activity";
import {
  presentToolActivity,
  selectVisibleToolActivities,
  toolActivityErrorSummary,
} from "./tool-activity-presentation";

function activity(id: string, overrides: Partial<ToolActivityItem> = {}): ToolActivityItem {
  return { id, name: "read_file", status: "success", ...overrides };
}

describe("presentToolActivity", () => {
  it("live 对象参数和历史 JSON 参数具有相同的自然语言展示", () => {
    const input = { path: "src/App.tsx" };
    const expected = { action: "读取文件", target: "src/App.tsx" };
    expect(presentToolActivity({ name: "read_file", input })).toEqual(expected);
    expect(presentToolActivity({ name: "read_file", input: JSON.stringify(input) })).toEqual(
      expected,
    );
  });

  it("展示批量目标数量，命令折成一行且有界", () => {
    expect(presentToolActivity({ name: "grep", input: { patterns: ["render", "tool"] } })).toEqual({
      action: "搜索内容",
      target: "render 等 2 项",
    });
    const { target } = presentToolActivity({
      name: "local_zsh",
      input: { command: `pnpm test\n${"x".repeat(200)}` },
    });
    expect(target).toHaveLength(100);
    expect(target).not.toContain("\n");
    expect(target?.endsWith("…")).toBe(true);
  });

  it("概览 URL 不带凭据、查询串和片段", () => {
    expect(
      presentToolActivity({
        name: "browser_open_url",
        input: { url: "https://user:secret@example.com/docs?token=private#secret" },
      }),
    ).toEqual({ action: "打开网页", target: "example.com/docs" });
  });

  it("尚未完成的 JSON 与未知参数保留动作，不编造目标", () => {
    expect(presentToolActivity({ name: "read_file", input: '{"path":' })).toEqual({
      action: "读取文件",
      target: undefined,
    });
    expect(
      presentToolActivity({ name: "read_file", input: { path: { nested: "wrong" } } }).target,
    ).toBeUndefined();
    expect(presentToolActivity({ name: "mcp__docs__query", input: { query: "React" } })).toEqual({
      action: "访问外部工具",
      target: "React",
    });
    expect(presentToolActivity({ name: "constructor" }).action).toBe("调用工具");
  });

  it("工作流从结构化定义取得标题，不把内部 ID 当作计划名", () => {
    expect(
      presentToolActivity({
        name: "submit_workflow",
        input: { definition: { title: "重构消息流" } },
      }),
    ).toEqual({ action: "生成工作流", target: "重构消息流" });
  });
});

describe("selectVisibleToolActivities", () => {
  it("第二、第三项到来不会触发整组隐藏", () => {
    const items = [activity("a"), activity("b"), activity("c")];
    expect(selectVisibleToolActivities(items.slice(0, 2), false)).toHaveLength(2);
    expect(selectVisibleToolActivities(items, false)).toHaveLength(3);
  });

  it("默认只显示最近 3 项已完成活动，展开后保持完整顺序", () => {
    const items = Array.from({ length: 8 }, (_, index) => activity(String(index)));
    expect(selectVisibleToolActivities(items, false).map((item) => item.id)).toEqual([
      "5",
      "6",
      "7",
    ]);
    expect(selectVisibleToolActivities(items, true)).toBe(items);
  });

  it("较早的失败、运行、等待、工作流和用户正在阅读的详情始终露出且不重复", () => {
    const items = [
      activity("failed", { status: "error" }),
      activity("running", { status: "running" }),
      activity("planned", { planned: true }),
      activity("workflow", { name: "submit_workflow", output: "已创建 plan_id=plan_1" }),
      activity("reading"),
      activity("hidden"),
      activity("recent1"),
      activity("recent2"),
      activity("recent3"),
    ];
    expect(
      selectVisibleToolActivities(items, false, new Set(["reading", "recent1"])).map(
        (item) => item.id,
      ),
    ).toEqual([
      "failed",
      "running",
      "planned",
      "workflow",
      "reading",
      "recent1",
      "recent2",
      "recent3",
    ]);
  });

  it("工作流未返回有效入口时无需永久保留成功的调用", () => {
    const items = [
      activity("no-plan", { name: "submit_workflow", output: "" }),
      activity("a"),
      activity("b"),
      activity("c"),
    ];
    expect(selectVisibleToolActivities(items, false).map((item) => item.id)).toEqual([
      "a",
      "b",
      "c",
    ]);
  });
});

describe("toolActivityErrorSummary", () => {
  it("失败即显示有界摘要，完整错误保留在原数据中", () => {
    const item = activity("error", { status: "error", errorText: `连接失败\n${"x".repeat(400)}` });
    expect(toolActivityErrorSummary(item)).toHaveLength(180);
    expect(toolActivityErrorSummary(item)).toMatch(/^连接失败 /);
    expect(item.errorText).toContain("\n");
  });

  it("错误字段缺省时仍展示失败与原输出；成功不误报", () => {
    expect(
      toolActivityErrorSummary(activity("a", { status: "error", output: "Permission denied" })),
    ).toBe("Permission denied");
    expect(toolActivityErrorSummary(activity("a", { status: "error" }))).toBe(
      "这一步未能完成，展开查看详情",
    );
    expect(toolActivityErrorSummary(activity("a", { output: "error count: 0" }))).toBeUndefined();
  });
});

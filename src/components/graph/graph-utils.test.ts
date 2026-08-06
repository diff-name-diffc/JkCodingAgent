import { describe, expect, it } from "vitest";

import type { AgentActivity } from "../../types";
import {
  buildToolCallEntries,
  formatCharCount,
  formatToolPayload,
  normalizeToolCallStatus,
} from "./graph-utils";

function activity(patch: Partial<AgentActivity>): AgentActivity {
  return {
    id: "act-1",
    runId: "run-1",
    nodeId: "node-1",
    sequence: 10,
    kind: "tool_call",
    status: "finished",
    title: "read_file_content",
    content: "输出文本",
    payloadJson: "{}",
    startedAt: 1000,
    finishedAt: 3500,
    ...patch,
  };
}

describe("formatToolPayload", () => {
  it("对象美化为缩进 JSON", () => {
    expect(formatToolPayload({ path: "src/a.ts", lines: 2 })).toBe(
      '{\n  "path": "src/a.ts",\n  "lines": 2\n}',
    );
  });

  it("JSON 字符串解析后还原换行与转义", () => {
    expect(formatToolPayload('{"cmd":"ls\\n-l"}')).toBe('{\n  "cmd": "ls\\n-l"\n}');
  });

  it("嵌套 JSON 字符串解包为纯文本", () => {
    expect(formatToolPayload('"第一行\\n第二行"')).toBe("第一行\n第二行");
  });

  it("普通文本原样返回", () => {
    expect(formatToolPayload("hello world")).toBe("hello world");
  });

  it("null/undefined 返回空串", () => {
    expect(formatToolPayload(null)).toBe("");
    expect(formatToolPayload(undefined)).toBe("");
  });
});

describe("formatCharCount", () => {
  it("千位以下原样", () => {
    expect(formatCharCount(0)).toBe("0");
    expect(formatCharCount(999)).toBe("999");
  });

  it("千位以上压缩为 k", () => {
    expect(formatCharCount(1234)).toBe("1.2k");
    expect(formatCharCount(12_345)).toBe("12k");
  });
});

describe("normalizeToolCallStatus", () => {
  it("finished/failed 映射为终态，其余视为执行中", () => {
    expect(normalizeToolCallStatus("finished")).toBe("succeeded");
    expect(normalizeToolCallStatus("failed")).toBe("failed");
    expect(normalizeToolCallStatus("started")).toBe("running");
    expect(normalizeToolCallStatus("updated")).toBe("running");
  });
});

describe("buildToolCallEntries", () => {
  it("只保留 tool_call 活动并按 sequence 排序", () => {
    const entries = buildToolCallEntries([
      activity({ id: "b", sequence: 20, title: "b_tool" }),
      activity({ id: "a", sequence: 10, title: "a_tool" }),
      activity({ id: "c", sequence: 30, kind: "assistant_text", title: "响应" }),
    ]);
    expect(entries.map((entry) => entry.name)).toEqual(["a_tool", "b_tool"]);
  });

  it("从 payload.args 提取输入并统计字符数", () => {
    const entries = buildToolCallEntries([
      activity({ payloadJson: JSON.stringify({ kind: "tool_call", args: { path: "a.ts" } }) }),
    ]);
    expect(entries[0].inputFormatted).toBe('{\n  "path": "a.ts"\n}');
    expect(entries[0].inputChars).toBe(JSON.stringify({ path: "a.ts" }).length);
  });

  it("优先用 content 作为输出，缺失时回退 payload.result", () => {
    const fromContent = buildToolCallEntries([activity({ content: "正文" })]);
    expect(fromContent[0].outputFormatted).toBe("正文");
    expect(fromContent[0].outputChars).toBe("正文".length);

    const fromPayload = buildToolCallEntries([
      activity({ content: "", payloadJson: JSON.stringify({ result: "回退" }) }),
    ]);
    expect(fromPayload[0].outputFormatted).toBe("回退");
  });

  it("计算耗时；未结束时为 null", () => {
    const finished = buildToolCallEntries([activity({ startedAt: 1000, finishedAt: 3500 })]);
    expect(finished[0].durationMs).toBe(2500);

    const running = buildToolCallEntries([activity({ status: "started", finishedAt: null })]);
    expect(running[0].durationMs).toBeNull();
    expect(running[0].status).toBe("running");
  });

  it("payload 损坏不影响卡片生成", () => {
    const entries = buildToolCallEntries([activity({ payloadJson: "不是 JSON" })]);
    expect(entries[0].inputChars).toBe(0);
    expect(entries[0].inputFormatted).toBe("");
  });
});

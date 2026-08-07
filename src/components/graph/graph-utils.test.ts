import { describe, expect, it } from "vitest";

import type { AgentActivity } from "../../types";
import {
  buildExecutionTimeline,
  buildNodeNotices,
  buildToolCallEntries,
  formatCharCount,
  formatContextUsage,
  formatToolPayload,
  latestContextUsage,
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

describe("buildNodeNotices", () => {
  it("compaction 活动转为压缩通知", () => {
    const notices = buildNodeNotices([
      activity({ id: "c1", kind: "compaction", status: "started", sequence: 20, content: "", payloadJson: JSON.stringify({ reason: "阈值触发" }) }),
      activity({ id: "c2", kind: "compaction", status: "finished", sequence: 30, content: "" }),
    ]);
    expect(notices.map((notice) => notice.title)).toEqual(["正在压缩上下文…", "上下文已压缩"]);
    expect(notices[0].detail).toBe("阈值触发");
    // status 归一化为工具卡片同口径（started→running / finished→succeeded），
    // 供样式修饰类区分「进行中 / 已完成 / 失败」。
    expect(notices.map((notice) => notice.status)).toEqual(["running", "succeeded"]);
  });

  it("retry 活动带尝试次数，失败标注错误", () => {
    const notices = buildNodeNotices([
      activity({ id: "r1", kind: "retry", status: "started", sequence: 40, content: "", payloadJson: JSON.stringify({ attempt: 1, maxAttempts: 2, errorMessage: "超时" }) }),
      activity({ id: "r2", kind: "retry", status: "failed", sequence: 50, content: "最终失败" }),
    ]);
    expect(notices[0].title).toBe("自动重试（1/2）");
    expect(notices[0].detail).toBe("超时");
    expect(notices[1].title).toBe("重试失败");
    expect(notices[1].detail).toBe("最终失败");
  });

  it("忽略工具调用与上下文占用活动", () => {
    expect(buildNodeNotices([activity({ kind: "tool_call" }), activity({ kind: "context_usage" })])).toEqual([]);
  });
});

describe("buildExecutionTimeline", () => {
  it("工具调用与通知按 sequence 混排", () => {
    const { timelineRows } = buildExecutionTimeline([
      activity({ id: "t1", kind: "tool_call", sequence: 10 }),
      activity({ id: "c1", kind: "compaction", status: "started", sequence: 5 }),
    ]);
    expect(timelineRows.map((row) => row.kind)).toEqual(["notice", "tool"]);
  });
});

describe("latestContextUsage / formatContextUsage", () => {
  it("取最后一次读数并格式化", () => {
    const reading = latestContextUsage([
      activity({ kind: "context_usage", sequence: 1, payloadJson: JSON.stringify({ tokens: 20_000, contextWindow: 128_000, percent: 15.6 }) }),
      activity({ kind: "context_usage", sequence: 2, payloadJson: JSON.stringify({ tokens: 55_000, contextWindow: 128_000, percent: 42.97 }) }),
    ]);
    expect(reading).toEqual({ tokens: 55_000, contextWindow: 128_000, percent: 42.97 });
    expect(formatContextUsage(reading!)).toBe("43.0% · 55k/128k");
  });

  it("compaction 后 tokens 为 null 时提示重新估算", () => {
    const reading = latestContextUsage([
      activity({ kind: "context_usage", payloadJson: JSON.stringify({ tokens: null, contextWindow: 128_000, percent: null }) }),
    ]);
    expect(formatContextUsage(reading!)).toBe("重新估算中…");
  });

  it("contextWindow 缺失或为 0 时只显示百分比", () => {
    const missing = latestContextUsage([
      activity({ kind: "context_usage", payloadJson: JSON.stringify({ tokens: 55_000, percent: 43.21 }) }),
    ]);
    expect(formatContextUsage(missing!)).toBe("43.2%");

    const zero = latestContextUsage([
      activity({ kind: "context_usage", payloadJson: JSON.stringify({ tokens: 55_000, contextWindow: 0, percent: 43.21 }) }),
    ]);
    expect(formatContextUsage(zero!)).toBe("43.2%");
  });

  it("从未上报时返回 null", () => {
    expect(latestContextUsage([activity({ kind: "tool_call" })])).toBeNull();
  });
});

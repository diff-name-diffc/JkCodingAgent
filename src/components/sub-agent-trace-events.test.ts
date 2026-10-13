import { describe, expect, it } from "vitest";
import type { SubAgentEvent, SubAgentEventPayload } from "../types";
import { mergeSubAgentTraceEvents, normalizeSubAgentUsage } from "./sub-agent-trace-events";

describe("sub-agent wire contract", () => {
  it("兼容旧 Rust snake_case 用量，同时保留新 camelCase", () => {
    const expected = { promptTokens: 7, completionTokens: 3, totalTokens: 10 };
    expect(
      normalizeSubAgentUsage({ prompt_tokens: 7, completion_tokens: 3, total_tokens: 10 }),
    ).toEqual(expected);
    expect(normalizeSubAgentUsage(expected)).toEqual(expected);
  });

  it("同毫秒同一事件按内容去重，不受字段和嵌套字段顺序影响", () => {
    const checkpoint: SubAgentEvent = JSON.parse(
      '{"event":"ToolStarted","timestampMs":10,"data":{"agentId":"browser-agent","arguments":{"index":1,"text":"input"},"taskId":"a","toolName":"input"}}',
    );
    const live: SubAgentEventPayload = JSON.parse(
      '{"sessionId":"s","toolCallId":"t","timestampMs":10,"event":"ToolStarted","data":{"toolName":"input","taskId":"a","arguments":{"text":"input","index":1},"agentId":"browser-agent"}}',
    );
    expect(mergeSubAgentTraceEvents([checkpoint], [live])).toHaveLength(1);
  });

  it("旧 snake_case checkpoint 与新 camelCase 实时用量规范化后去重", () => {
    const checkpoint: SubAgentEvent = JSON.parse(
      '{"event":"UsageUpdated","timestampMs":10,"data":{"tokenUsage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}}}',
    );
    const live: SubAgentEventPayload = {
      sessionId: "s",
      toolCallId: "t",
      timestampMs: 10,
      event: "UsageUpdated",
      data: { tokenUsage: { promptTokens: 7, completionTokens: 3, totalTokens: 10 } },
    };
    expect(mergeSubAgentTraceEvents([checkpoint], [live])).toHaveLength(1);
  });

  it("同毫秒重复的文本增量只消除跨来源重叠，不丢失真实重复", () => {
    const event: SubAgentEventPayload = {
      sessionId: "s",
      toolCallId: "t",
      timestampMs: 10,
      event: "llmDelta",
      data: { delta: "哈" },
    };
    expect(mergeSubAgentTraceEvents([event, event], [event, event])).toHaveLength(2);
    expect(mergeSubAgentTraceEvents([event], [event, event])).toHaveLength(2);
    expect(mergeSubAgentTraceEvents([], [event, event])).toHaveLength(2);
  });

  it("普通 Rig 终态快照的聚合文本不与原始实时增量重复叠加", () => {
    const checkpoint: SubAgentEvent[] = [
      { event: "llmDelta", timestampMs: 10, data: { delta: "完整文本" } },
      { event: "Finished", timestampMs: 11, data: { result: "完成" } },
    ];
    const live: SubAgentEventPayload[] = ["完整", "文本"].map((delta) => ({
      sessionId: "s",
      toolCallId: "t",
      timestampMs: 10,
      event: "llmDelta",
      data: { delta },
    }));
    expect(mergeSubAgentTraceEvents(checkpoint, live)).toEqual(checkpoint);
  });
});

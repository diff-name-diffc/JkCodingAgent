import { describe, expect, it } from "vitest";
import {
  appendAssistantTextSegment,
  appendToolSummarySegment,
  demoteActiveTextSegments,
  pushAssistantSegment,
  type AssistantTurnSegment,
} from "./assistant-segments";

describe("助手轮次分段身份", () => {
  it("同一消息的增量累积，不同消息即使文字一致也保留独立分段", () => {
    let segments = appendAssistantTextSegment([], "检查", "message-1");
    segments = appendAssistantTextSegment(segments, "项目", "message-1");
    segments = appendAssistantTextSegment(segments, "检查项目", "message-2");
    expect(segments).toEqual([
      { kind: "assistant-text", text: "检查项目", messageId: "message-1" },
      { kind: "assistant-text", text: "检查项目", messageId: "message-2" },
    ]);
  });

  it("切换模型调用只标记阶段说明，原有文字和消息 ID 均保留", () => {
    const original = appendAssistantTextSegment([], "先检查项目", "message-1");
    const next = appendAssistantTextSegment(
      demoteActiveTextSegments(original),
      "检查完成",
      "message-2",
    );
    expect(next).toEqual([
      { kind: "assistant-text", text: "先检查项目", messageId: "message-1", superseded: true },
      { kind: "assistant-text", text: "检查完成", messageId: "message-2" },
    ]);
    expect(original[0].superseded).toBeUndefined();
  });

  it("历史相邻阶段说明不因相同 superseded 标记而粘成一段", () => {
    const segments: AssistantTurnSegment[] = [];
    for (const messageId of ["message-1", "message-2"]) {
      pushAssistantSegment(segments, {
        kind: "assistant-text",
        text: "检查中",
        messageId,
        superseded: true,
      });
    }
    expect(segments.map((segment) => segment.messageId)).toEqual(["message-1", "message-2"]);
  });

  it("同一工具摘要累积，不同工具的摘要不合并", () => {
    let segments: AssistantTurnSegment[] = [];
    for (const [toolCallId, delta] of [
      ["call-1", "读取"],
      ["call-1", "成功"],
      ["call-2", "完成"],
    ]) {
      segments = appendToolSummarySegment(segments, {
        toolCallId,
        delta,
        name: "read_file",
        resultMode: "summary",
      });
    }
    expect(segments.map((segment) => segment.text)).toEqual(["读取成功", "完成"]);
  });

  it("并行工具摘要穿插正文时，两类增量各自累积且不产生重复消息身份", () => {
    let segments = appendAssistantTextSegment([], "已经", "message-1");
    segments = appendToolSummarySegment(segments, {
      toolCallId: "call-1",
      name: "read_file",
      delta: "读取",
      resultMode: "summary",
    });
    segments = appendAssistantTextSegment(segments, "完成", "message-1");
    segments = appendToolSummarySegment(segments, {
      toolCallId: "call-1",
      name: "read_file",
      delta: "成功",
      resultMode: "summary",
    });
    expect(segments.map((segment) => segment.text)).toEqual(["已经完成", "读取成功"]);
  });
});

describe("无身份载荷的相邻合并兼容分支", () => {
  it("两个无身份正文增量相邻时合并为一段", () => {
    let segments = appendAssistantTextSegment([], "切换模型", undefined);
    segments = appendAssistantTextSegment(segments, "后继续", undefined);
    expect(segments).toHaveLength(1);
    expect(segments[0].text).toBe("切换模型后继续");
  });

  it("无身份增量紧跟工具摘要时新起一段而非并入", () => {
    let segments = appendToolSummarySegment([], {
      toolCallId: "call-1",
      name: "read_file",
      delta: "读取成功",
      resultMode: "summary",
    });
    segments = appendAssistantTextSegment(segments, "接下来", undefined);
    expect(segments).toHaveLength(2);
    expect(segments[0].kind).toBe("tool-summary");
    expect(segments[1].kind).toBe("assistant-text");
    expect(segments[1].text).toBe("接下来");
  });

  it("无身份增量不并入带身份段落", () => {
    let segments = appendAssistantTextSegment([], "正文", "message-1");
    segments = appendAssistantTextSegment(segments, "无身份说明", undefined);
    expect(segments.map((segment) => segment.text)).toEqual(["正文", "无身份说明"]);
    expect(segments[0].messageId).toBe("message-1");
    expect(segments[1].messageId).toBeUndefined();
  });
});

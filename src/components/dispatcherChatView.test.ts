import { describe, expect, it } from "vitest";
import type { DispatcherMessage } from "../types";
import { buildDispatcherDisplayItems } from "./dispatcherChatView";
import { finishLiveToolActivity } from "./dispatcher-chat/live-tool-activity";

describe("dispatcher tool result visibility", () => {
  it("历史工具卡片展示 Agent 实际收到的 contextPayload", () => {
    const messages: DispatcherMessage[] = [
      {
        id: "assistant",
        workspaceId: "workspace",
        role: "assistant",
        segments: [],
        content: "",
        toolCallsJson: JSON.stringify([
          { id: "call-1", function: { name: "read_dwg", arguments: "{}" } },
        ]),
        createdAt: "2026-08-27T00:00:00Z",
      },
      {
        id: "tool",
        workspaceId: "workspace",
        role: "tool",
        segments: [],
        content: "共提取 49 个图框。",
        contextPayload: "frame 37: A-01 总说明\nframe 38: A-02 系统图",
        toolCallId: "call-1",
        toolName: "read_dwg",
        toolResultMode: "intent_compressed",
        createdAt: "2026-08-27T00:00:01Z",
      },
    ];

    const items = buildDispatcherDisplayItems(messages);
    expect(items[0]).toMatchObject({
      kind: "assistant",
      turn: {
        tools: [{ output: "frame 37: A-01 总说明\nframe 38: A-02 系统图" }],
        segments: [{ kind: "tool-summary", text: "共提取 49 个图框。" }],
      },
    });
  });

  it("实时工具卡片展示 ToolFinished 中的 contextPayload", () => {
    const tools = finishLiveToolActivity([], {
      toolCallId: "call-1",
      name: "read_dwg",
      arguments: "{}",
      displayText: "共提取 49 个图框。",
      contextPayload: "frame 37: A-01 总说明\nframe 38: A-02 系统图",
      resultMode: "intent_compressed",
      detailRefs: [],
    });

    expect(tools[0].output).toBe("frame 37: A-01 总说明\nframe 38: A-02 系统图");
  });
});

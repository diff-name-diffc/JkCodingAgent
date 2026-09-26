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


it("accepted 保持运行状态，乱序完成按 task_id 更新原卡片", () => {
  const base = { workspaceId: "workspace", segments: [], content: "", createdAt: "2026-09-26T00:00:00Z" };
  const messages: DispatcherMessage[] = [
    { ...base, id: "calls", role: "assistant", toolCallsJson: JSON.stringify([
      { id: "a", function: { name: "read_file", arguments: "{}" } },
      { id: "b", function: { name: "read_file", arguments: "{}" } },
    ]) },
    ...["a", "b"].map((id): DispatcherMessage => ({ ...base, id: "reply-" + id, role: "tool",
      toolCallId: id, toolTaskId: "task-" + id, toolName: "read_file", toolResultMode: "accepted" })),
  ];
  expect(buildDispatcherDisplayItems(messages)[0]).toMatchObject({
    turn: { tools: [{ id: "a", status: "running" }, { id: "b", status: "running" }] },
  });
  for (const id of ["b", "a"]) messages.push({
    ...base, id: "completion-" + id, role: "runtime", toolTaskId: "task-" + id,
    contextPayload: JSON.stringify({ kind: "tool_completion", status: "succeeded", context_payload: "result-" + id }),
  });
  expect(buildDispatcherDisplayItems(messages)[0]).toMatchObject({
    turn: { tools: [{ id: "a", status: "success", output: "result-a" }, { id: "b", status: "success", output: "result-b" }] },
  });
});

it("跨回合归并：停止后的完成消息写回发起工具的原始回合，不造幽灵回合", () => {
  const base = { workspaceId: "workspace", segments: [] as DispatcherMessage["segments"] };
  const messages: DispatcherMessage[] = [
    {
      ...base, id: "calls", role: "assistant", content: "", createdAt: "2026-09-26T00:00:00Z",
      toolCallsJson: JSON.stringify([{ id: "a", function: { name: "local_zsh", arguments: "{}" } }]),
    },
    {
      ...base, id: "reply-a", role: "tool", content: "", createdAt: "2026-09-26T00:00:01Z",
      toolCallId: "a", toolTaskId: "task-a", toolName: "local_zsh", toolResultMode: "accepted",
    },
    // 用户立即发送新消息：之后的清理完成消息按 createdAt 落在新 user 之后。
    { ...base, id: "user-2", role: "user", content: "继续", createdAt: "2026-09-26T00:00:05Z" },
    {
      ...base, id: "completion-a", role: "runtime", content: "", createdAt: "2026-09-26T00:00:06Z",
      toolTaskId: "task-a",
      contextPayload: JSON.stringify({ kind: "tool_completion", status: "succeeded", context_payload: "done" }),
    },
  ];

  const items = buildDispatcherDisplayItems(messages);
  expect(items.map((item) => item.kind)).toEqual(["assistant", "user"]);
  const turn = items[0].kind === "assistant" ? items[0].turn : null;
  // 卡片在原始回合完成，且耗时为完成时刻 − 开始时刻（覆盖 accepted 的部分值）。
  expect(turn?.tools).toMatchObject([{ id: "a", status: "success", output: "done", durationMs: 6000 }]);
});

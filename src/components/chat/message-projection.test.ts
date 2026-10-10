import { describe, expect, it } from "vitest";
import type { DispatcherMessage } from "../../types";
import { createIdleLiveSessionState } from "../dispatcherSessionStore";
import { buildItems, projectLiveMessageItems, type MessageDisplayItem } from "./message-projection";

const user: DispatcherMessage = {
  id: "user-1",
  workspaceId: "session-1",
  role: "user",
  content: "检查项目",
  segments: [],
  createdAt: "2026-10-10T00:00:00Z",
};
function assistant(
  id: string,
  content: string,
  extra: Partial<DispatcherMessage> = {},
): DispatcherMessage {
  return { ...user, role: "assistant", id, content, ...extra };
}
function lastAssistant(items: MessageDisplayItem[]) {
  const item = items[items.length - 1];
  if (item.kind !== "assistant") throw new Error("缺少助手轮次");
  return item;
}
const live = () => ({ ...createIdleLiveSessionState(), isLoading: true });

describe("同一助手轮次从实时内容交接到历史", () => {
  it("快速工具轮次仅更新同一行，完成时保留全部阶段说明与稳定身份", () => {
    const state = {
      ...live(),
      streamingSegments: [
        { kind: "assistant-text" as const, messageId: "a1", text: "先读取配置", superseded: true },
        { kind: "assistant-text" as const, messageId: "a2", text: "已找到问题" },
      ],
      liveToolCalls: [
        { id: "call-1", name: "read_file", status: "success" as const, output: "配置内容" },
      ],
    };
    const first = lastAssistant(projectLiveMessageItems(buildItems([user]), state, "session-1"));
    const persisted = buildItems([
      user,
      assistant("a1", "先读取配置", {
        toolCallsJson: JSON.stringify([
          { id: "call-1", function: { name: "read_file", arguments: "{}" } },
        ]),
      }),
      assistant("tool-1", "配置内容", {
        role: "tool",
        toolCallId: "call-1",
        toolName: "read_file",
      }),
      assistant("a2", "已找到问题"),
    ]);
    const during = projectLiveMessageItems(persisted, state, "session-1");
    const complete = projectLiveMessageItems(persisted, createIdleLiveSessionState(), "session-1");
    expect(during).toHaveLength(2);
    expect(lastAssistant(during).segments.map((segment) => segment.text)).toEqual([
      "先读取配置",
      "已找到问题",
    ]);
    expect(lastAssistant(during).tools).toHaveLength(1);
    expect(first.id).toBe(lastAssistant(during).id);
    expect(first.id).toBe(lastAssistant(complete).id);
  });

  it("相同正文但不同消息 ID 不做内容去重，也不粘连相邻历史草稿", () => {
    const history = buildItems([user, assistant("a1", "检查中"), assistant("a2", "检查中")]);
    const items = projectLiveMessageItems(
      history,
      {
        ...live(),
        streamingSegments: [{ kind: "assistant-text", messageId: "a2", text: "检查中" }],
      },
      "session-1",
    );
    expect(lastAssistant(items).segments.map((segment) => segment.messageId)).toEqual(["a1", "a2"]);
  });

  it("已落库的完整正文不被实时前缀覆盖", () => {
    const history = buildItems([user, assistant("a1", "检查已经完成")]);
    const items = projectLiveMessageItems(
      history,
      {
        ...live(),
        streamingSegments: [{ kind: "assistant-text", messageId: "a1", text: "检查" }],
      },
      "session-1",
    );
    expect(lastAssistant(items).segments.map((segment) => segment.text)).toEqual(["检查已经完成"]);
  });

  it("落库只是流式前缀时保留已显示的后续增量", () => {
    const history = buildItems([user, assistant("a1", "检查")]);
    const items = projectLiveMessageItems(
      history,
      {
        ...live(),
        streamingSegments: [{ kind: "assistant-text", messageId: "a1", text: "检查已经完成" }],
      },
      "session-1",
    );
    expect(lastAssistant(items).segments.map((segment) => segment.text)).toEqual(["检查已经完成"]);
  });

  it("工具摘要以调用 ID 去重，仍保留工具结果与摘要各自用途", () => {
    const history = buildItems([
      user,
      assistant("t1", "读取了三份文件", {
        role: "tool",
        toolCallId: "call-1",
        toolName: "read_file",
        toolResultMode: "summary",
        contextPayload: "完整模型输入",
      }),
    ]);
    const items = projectLiveMessageItems(
      history,
      {
        ...live(),
        streamingSegments: [
          {
            kind: "tool-summary",
            toolCallId: "call-1",
            text: "读取了三份文件",
            resultMode: "summary",
          },
        ],
        liveToolCalls: [{ id: "call-1", name: "read_file", status: "running" }],
      },
      "session-1",
    );
    expect(lastAssistant(items).segments).toHaveLength(1);
    expect(lastAssistant(items).tools).toMatchObject([
      { id: "call-1", status: "success", output: "完整模型输入" },
    ]);
  });

  it("推理快照按消息 ID 去重；后续消息相同推理文字仍属于新阶段", () => {
    const history = buildItems([
      user,
      assistant("a1", "", { thinkingContent: "检查依赖", thinkingElapsedMs: 100 }),
    ]);
    const first = projectLiveMessageItems(
      history,
      {
        ...live(),
        liveThinking: { messageId: "a1", text: "检查依赖", elapsedMs: 100 },
      },
      "session-1",
    );
    expect(lastAssistant(first).thinking).toEqual({ text: "检查依赖", elapsedMs: 100 });
    expect(lastAssistant(first).isThinking).toBe(false);
    const second = projectLiveMessageItems(
      history,
      {
        ...live(),
        liveThinking: { messageId: "a2", text: "检查依赖", elapsedMs: 200 },
      },
      "session-1",
    );
    expect(lastAssistant(second).thinking).toEqual({
      text: "检查依赖\n\n检查依赖",
      elapsedMs: 300,
    });
    expect(lastAssistant(second).isThinking).toBe(true);
  });

  it("正文开始输出后停止显示思考进行态，但保留思考内容", () => {
    const items = projectLiveMessageItems(
      buildItems([user]),
      {
        ...live(),
        liveThinking: { messageId: "a1", text: "检查依赖", elapsedMs: 200 },
        streamingSegments: [{ kind: "assistant-text", messageId: "a1", text: "检查完毕" }],
      },
      "session-1",
    );
    expect(lastAssistant(items)).toMatchObject({
      isThinking: false,
      thinking: { text: "检查依赖" },
    });
  });

  it("失败或停止后的未对账内容仍可读，不再显示流式状态", () => {
    const items = projectLiveMessageItems(
      buildItems([user]),
      {
        ...createIdleLiveSessionState(),
        runError: "已停止",
        streamingSegments: [{ kind: "assistant-text", messageId: "a1", text: "已完成的检查" }],
      },
      "session-1",
    );
    expect(lastAssistant(items)).toMatchObject({
      isStreaming: false,
      segments: [{ text: "已完成的检查" }],
    });
  });

  it("当前轮次更新不改变更早轮次的对象身份", () => {
    const history = buildItems([user, assistant("a1", "第一轮答复"), { ...user, id: "user-2" }]);
    const items = projectLiveMessageItems(
      history,
      { ...live(), assistantPlaceholder: "正在思考" },
      "session-1",
    );
    expect(items[0]).toBe(history[0]);
    expect(items[1]).toBe(history[1]);
    expect(lastAssistant(items).id).toBe("assistant-turn-user-2");
  });

  it("正文增量不重建未变化的工具卡片，工具快照更新才重新投影", () => {
    const history = buildItems([
      user,
      assistant("a1", "先读取文件", {
        toolCallsJson: JSON.stringify([
          { id: "call-1", function: { name: "read_file", arguments: "{}" } },
        ]),
      }),
    ]);
    const firstState = {
      ...live(),
      liveToolCalls: [{ id: "call-1", name: "read_file", status: "running" as const }],
    };
    const first = lastAssistant(projectLiveMessageItems(history, firstState, "session-1"));
    const second = lastAssistant(
      projectLiveMessageItems(
        history,
        {
          ...firstState,
          streamingSegments: [{ kind: "assistant-text", messageId: "a2", text: "已看到" }],
        },
        "session-1",
      ),
    );
    expect(second.tools).toBe(first.tools);
    expect(second.tools[0]).toBe(first.tools[0]);
    const third = lastAssistant(
      projectLiveMessageItems(
        history,
        {
          ...firstState,
          liveToolCalls: [{ ...firstState.liveToolCalls[0], status: "success", output: "完成" }],
        },
        "session-1",
      ),
    );
    expect(third.tools).not.toBe(first.tools);
    expect(third.tools[0].status).toBe("success");
  });

  it("切换会话不会把新实时输出接到旧会话的最后一轮", () => {
    const history = buildItems([user, assistant("a1", "旧会话")]);
    expect(projectLiveMessageItems(history, live(), "session-2")).toBe(history);
    const nextHistory = buildItems([{ ...user, workspaceId: "session-2", id: "user-2" }]);
    expect(lastAssistant(projectLiveMessageItems(nextHistory, live(), "session-2")).id).toBe(
      "assistant-turn-user-2",
    );
  });
});

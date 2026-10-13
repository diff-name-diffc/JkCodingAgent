import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { DispatcherMessage, DispatcherMessageWire } from "../../types";
import {
  cleanupDispatcherSession,
  createIdleLiveSessionState,
  getDispatcherLiveSessionState,
  getDispatcherSessionRunning,
  nextDispatcherActiveRunId,
  setDispatcherLiveSessionState,
  subscribeDispatcherLiveSession,
  subscribeDispatcherMessages,
} from "../dispatcherSessionStore";
import { createDispatcherEventChannel } from "./event-channel";
import { partitionAssistantSegments } from "./assistant-segments";
import { buildItems, projectLiveMessageItems } from "../chat/message-projection";
import { mergeDispatcherMessages } from "./dispatcherChatUtils";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  Channel: class {
    onmessage = vi.fn();
  },
}));

const sessionId = "event-channel-session";
const snapshot: DispatcherMessageWire = {
  id: "assistant-1",
  workspaceId: sessionId,
  role: "assistant",
  segmentsJson: JSON.stringify([{ type: "text", text: "先检查配置" }]),
  thinkingContent: "检查依赖",
  thinkingElapsedMs: 200,
  createdAt: "2026-10-10T00:00:00Z",
};
let releaseSubscription: () => void;

beforeEach(() => {
  vi.clearAllMocks();
  cleanupDispatcherSession(sessionId);
  releaseSubscription = subscribeDispatcherLiveSession(sessionId, () => {});
  setDispatcherLiveSessionState(sessionId, {
    ...createIdleLiveSessionState(),
    hasPendingRun: true,
    isLoading: true,
  });
  vi.mocked(invoke).mockImplementation(() => new Promise(() => {}));
});
afterEach(() => {
  releaseSubscription();
  cleanupDispatcherSession(sessionId);
  vi.restoreAllMocks();
});

function channel() {
  return createDispatcherEventChannel({
    targetSessionId: sessionId,
    runId: nextDispatcherActiveRunId(sessionId),
    updateLiveSessionState: (id, updater) => {
      setDispatcherLiveSessionState(
        id,
        updater(getDispatcherLiveSessionState(id) ?? createIdleLiveSessionState()),
      );
    },
    refreshSessionTokenUsage: async () => {},
  });
}

describe("流式内容交接事件", () => {
  it("模型切换通知明确属于过程说明，后续真实正文仍属于回答", () => {
    const events = channel();
    events.onmessage({
      event: "modelSwitched",
      data: { fromModel: "chat-model", toModel: "vision-model", reason: "检测到图片" },
    });
    events.onmessage({ event: "assistantStarted", data: { messageId: "answer-1" } });
    events.onmessage({
      event: "assistantDelta",
      data: { messageId: "answer-1", seq: 0, delta: "图中是项目架构" },
    });
    const { progressSegments, answerSegments } = partitionAssistantSegments(
      getDispatcherLiveSessionState(sessionId)!.streamingSegments,
    );
    expect(progressSegments).toEqual([
      {
        kind: "assistant-text",
        text: "> 检测到图片，已从 chat-model 自动切换到视觉模型 vision-model。\n\n",
        superseded: true,
      },
    ]);
    expect(answerSegments).toEqual([
      { kind: "assistant-text", messageId: "answer-1", text: "图中是项目架构" },
    ]);
  });

  it("无工具渐进回复不因下一次模型调用开始而降级，思考和正文仍保留", () => {
    const events = channel();
    events.onmessage({
      event: "assistantThinkingDelta",
      data: { messageId: "assistant-1", seq: 1, delta: "检查依赖", elapsedMs: 200 },
    });
    events.onmessage({
      event: "assistantDelta",
      data: { messageId: "assistant-1", seq: 2, delta: "先检查配置" },
    });
    events.onmessage({ event: "assistantMessage", data: { message: snapshot, lastSeq: 2 } });
    expect(getDispatcherLiveSessionState(sessionId)).toMatchObject({
      streamingSegments: [{ messageId: "assistant-1", text: "先检查配置" }],
      liveThinking: { messageId: "assistant-1", text: "检查依赖" },
    });
    expect(
      getDispatcherLiveSessionState(sessionId)?.streamingSegments[0].superseded,
    ).toBeUndefined();
    events.onmessage({ event: "assistantStarted", data: { messageId: "assistant-2" } });
    expect(getDispatcherLiveSessionState(sessionId)?.streamingSegments).toEqual([
      { kind: "assistant-text", messageId: "assistant-1", text: "先检查配置" },
    ]);
  });

  it("工具规划只标记当前明确的模型消息，保留之前的无工具渐进回复", () => {
    const events = channel();
    events.onmessage({ event: "assistantStarted", data: { messageId: "answer-1" } });
    events.onmessage({
      event: "assistantDelta",
      data: { messageId: "answer-1", seq: 0, delta: "已完成部分检查" },
    });
    events.onmessage({ event: "assistantStarted", data: { messageId: "progress-2" } });
    events.onmessage({
      event: "assistantDelta",
      data: { messageId: "progress-2", seq: 0, delta: "继续读取文件" },
    });
    events.onmessage({
      event: "toolPlanned",
      data: { toolCallId: "call-2", name: "read_file", arguments: "{}" },
    });
    const segments = getDispatcherLiveSessionState(sessionId)!.streamingSegments;
    expect(segments).toEqual([
      { kind: "assistant-text", messageId: "answer-1", text: "已完成部分检查" },
      {
        kind: "assistant-text",
        messageId: "progress-2",
        text: "继续读取文件",
        superseded: true,
      },
    ]);
    events.onmessage({ event: "failed", data: { workspaceId: sessionId, message: "执行失败" } });
    expect(getDispatcherLiveSessionState(sessionId)?.streamingSegments).toEqual(segments);
  });

  it("缺少 assistantStarted 关联时不根据最后一段猜测工具归属", () => {
    const events = channel();
    events.onmessage({
      event: "assistantDelta",
      data: { messageId: "answer-1", seq: 0, delta: "保留回复" },
    });
    events.onmessage({
      event: "toolPlanned",
      data: { toolCallId: "call-1", name: "read_file", arguments: "{}" },
    });
    expect(
      getDispatcherLiveSessionState(sessionId)?.streamingSegments[0].superseded,
    ).toBeUndefined();
  });

  it("当前普通回复以明确事件身份交接到落库 ID，重复文案的不同回复仍各保留一次", () => {
    let messages: DispatcherMessage[] = [];
    const observedTextCounts: number[] = [];
    const unsubscribe = subscribeDispatcherMessages(sessionId, (incoming) => {
      messages = mergeDispatcherMessages(messages, incoming);
      const item = projectLiveMessageItems(
        buildItems(messages),
        getDispatcherLiveSessionState(sessionId) ?? null,
        sessionId,
      )[0];
      if (item.kind !== "assistant") throw new Error("缺少助手轮次");
      observedTextCounts.push(item.segments.length);
    });
    const events = channel();
    for (const index of [1, 2]) {
      const messageId = `stream-${index}`;
      events.onmessage({ event: "assistantStarted", data: { messageId } });
      events.onmessage({
        event: "assistantThinkingDelta",
        data: { messageId, seq: 0, delta: "检查依赖", elapsedMs: 200 },
      });
      events.onmessage({
        event: "assistantDelta",
        data: { messageId, seq: 1, delta: "先检查配置" },
      });
      events.onmessage({
        event: "assistantMessage",
        data: {
          message: { ...snapshot, id: `saved-${index}`, createdAt: `2026-10-10T00:00:0${index}Z` },
          lastSeq: 1,
        },
      });
    }
    const state = getDispatcherLiveSessionState(sessionId)!;
    expect(observedTextCounts).toEqual([1, 2]);
    expect(state.streamingSegments).toEqual([
      { kind: "assistant-text", messageId: "saved-1", text: "先检查配置" },
      { kind: "assistant-text", messageId: "saved-2", text: "先检查配置" },
    ]);
    expect(state.liveThinking?.messageId).toBe("saved-2");
    const item = projectLiveMessageItems(buildItems(messages), state, sessionId)[0];
    if (item.kind !== "assistant") throw new Error("缺少助手轮次");
    expect(item.thinking).toEqual({ text: "检查依赖\n\n检查依赖", elapsedMs: 400 });
    expect(partitionAssistantSegments(item.segments).answerSegments).toHaveLength(2);
    unsubscribe();
  });

  it("无关联增量的合成收尾不改写当前未完成回复的身份", () => {
    const events = channel();
    events.onmessage({ event: "assistantStarted", data: { messageId: "stream-1" } });
    events.onmessage({
      event: "assistantDelta",
      data: { messageId: "stream-1", seq: 0, delta: "未完成的回复" },
    });
    events.onmessage({
      event: "assistantMessage",
      data: { message: { ...snapshot, id: "synthetic" }, lastSeq: null },
    });
    expect(getDispatcherLiveSessionState(sessionId)?.streamingSegments).toEqual([
      { kind: "assistant-text", messageId: "stream-1", text: "未完成的回复" },
    ]);
  });

  it("真实工具事件顺序与独立落库 ID 对账后，阶段说明和多条渐进回复分类一致", async () => {
    let resolveHistory!: (messages: DispatcherMessageWire[]) => void;
    vi.mocked(invoke).mockReturnValue(
      new Promise((resolve) => {
        resolveHistory = resolve;
      }),
    );
    let messages: DispatcherMessage[] = [];
    const unsubscribe = subscribeDispatcherMessages(sessionId, (incoming) => {
      messages = mergeDispatcherMessages(messages, incoming);
    });
    const events = channel();
    const contents = ["先读取文件", "已完成部分检查", "全部检查完成"];
    for (const [index, text] of contents.entries()) {
      const messageId = `stream-${index}`;
      events.onmessage({ event: "assistantStarted", data: { messageId } });
      events.onmessage({
        event: "assistantDelta",
        data: { messageId, seq: 0, delta: text },
      });
      if (index === 0) {
        events.onmessage({
          event: "toolPlanned",
          data: { toolCallId: "call-1", name: "read_file", arguments: "{}" },
        });
      }
    }
    const before = partitionAssistantSegments(
      getDispatcherLiveSessionState(sessionId)!.streamingSegments,
    );
    expect(before.progressSegments.map((segment) => segment.text)).toEqual([contents[0]]);
    expect(before.answerSegments.map((segment) => segment.text)).toEqual(contents.slice(1));
    events.onmessage({ event: "finished", data: { workspaceId: sessionId, messageCount: 3 } });
    resolveHistory(
      contents.map((text, index) => ({
        ...snapshot,
        id: `saved-${index}`,
        createdAt: `2026-10-10T00:00:0${index}Z`,
        segmentsJson: JSON.stringify([{ type: "text", text }]),
        toolCallsJson:
          index === 0
            ? JSON.stringify([{ id: "call-1", function: { name: "read_file", arguments: "{}" } }])
            : undefined,
      })),
    );
    await Promise.resolve();
    const history = projectLiveMessageItems(
      buildItems(messages),
      getDispatcherLiveSessionState(sessionId) ?? null,
      sessionId,
    );
    const turn = history[0];
    if (turn.kind !== "assistant") throw new Error("缺少助手轮次");
    const after = partitionAssistantSegments(turn.segments);
    expect(after.progressSegments.map((segment) => segment.text)).toEqual([contents[0]]);
    expect(after.answerSegments.map((segment) => segment.text)).toEqual(contents.slice(1));
    expect(turn.segments.map((segment) => segment.messageId)).toEqual([
      "saved-0",
      "saved-1",
      "saved-2",
    ]);
    unsubscribe();
  });

  it("完成立即停止运行，历史对账发布前保留全部实时内容", async () => {
    let resolveHistory!: (messages: DispatcherMessageWire[]) => void;
    vi.mocked(invoke).mockReturnValue(
      new Promise((resolve) => {
        resolveHistory = resolve;
      }),
    );
    const events = channel();
    events.onmessage({
      event: "assistantDelta",
      data: { messageId: "assistant-1", seq: 1, delta: "先检查配置" },
    });
    let liveLengthAtHistory = 0;
    const unsubscribe = subscribeDispatcherMessages(sessionId, () => {
      liveLengthAtHistory = getDispatcherLiveSessionState(sessionId)?.streamingSegments.length ?? 0;
    });
    events.onmessage({ event: "finished", data: { workspaceId: sessionId, messageCount: 1 } });
    expect(getDispatcherSessionRunning(sessionId)).toBe(false);
    expect(getDispatcherLiveSessionState(sessionId)?.streamingSegments).toHaveLength(1);
    resolveHistory([snapshot]);
    await Promise.resolve();
    expect(liveLengthAtHistory).toBe(1);
    expect(getDispatcherLiveSessionState(sessionId)).toEqual(createIdleLiveSessionState());
    unsubscribe();
  });

  it("失败交接同样保留正文，权威历史就绪后仍保留错误提示", async () => {
    let resolveHistory!: (messages: DispatcherMessageWire[]) => void;
    vi.mocked(invoke).mockReturnValue(
      new Promise((resolve) => {
        resolveHistory = resolve;
      }),
    );
    const events = channel();
    events.onmessage({
      event: "assistantDelta",
      data: { messageId: "assistant-1", seq: 1, delta: "已完成检查" },
    });
    events.onmessage({
      event: "failed",
      data: { workspaceId: sessionId, message: "工具执行失败" },
    });
    expect(getDispatcherLiveSessionState(sessionId)).toMatchObject({
      isLoading: false,
      runError: "工具执行失败",
      streamingSegments: [{ text: "已完成检查" }],
    });
    resolveHistory([snapshot]);
    await Promise.resolve();
    expect(getDispatcherLiveSessionState(sessionId)).toEqual({
      ...createIdleLiveSessionState(),
      runError: "工具执行失败",
    });
  });

  it("对账失败记录错误并保留可读内容，不悄悄清空当前轮次", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.mocked(invoke).mockRejectedValue(new Error("暂时无法读取历史"));
    const events = channel();
    events.onmessage({
      event: "assistantDelta",
      data: { messageId: "assistant-1", seq: 1, delta: "保留正文" },
    });
    events.onmessage({ event: "finished", data: { workspaceId: sessionId, messageCount: 1 } });
    await Promise.resolve();
    await Promise.resolve();
    expect(getDispatcherLiveSessionState(sessionId)?.streamingSegments).toMatchObject([
      { text: "保留正文" },
    ]);
    expect(error).toHaveBeenCalled();
  });

  it("旧对账返回时若新运行已经开始，不清空或覆盖新运行内容", async () => {
    let resolveHistory!: (messages: DispatcherMessageWire[]) => void;
    vi.mocked(invoke).mockReturnValue(
      new Promise((resolve) => {
        resolveHistory = resolve;
      }),
    );
    const events = channel();
    events.onmessage({ event: "finished", data: { workspaceId: sessionId, messageCount: 1 } });
    const next = {
      ...createIdleLiveSessionState(),
      isLoading: true,
      assistantPlaceholder: "新的运行",
    };
    // 不再设置新的 activeRunId：本用例专测 settleDispatcherRun 回调里的
    // streamingSegments 引用守卫（对账在途时 live 状态被新运行整体替换，
    // 旧收尾不得清空它），与 activeRunId 守卫（下个用例）分开覆盖。
    setDispatcherLiveSessionState(sessionId, next);
    resolveHistory([snapshot]);
    await Promise.resolve();
    expect(getDispatcherLiveSessionState(sessionId)).toBe(next);
  });

  it("对账在途期间开启新 run 时，过期全量快照不推送（activeRunId 守卫）", async () => {
    let resolveHistory!: (messages: DispatcherMessageWire[]) => void;
    vi.mocked(invoke).mockReturnValue(
      new Promise((resolve) => {
        resolveHistory = resolve;
      }),
    );
    const subscriber = vi.fn();
    releaseSubscription();
    releaseSubscription = subscribeDispatcherMessages(sessionId, subscriber);
    const events = channel();
    events.onmessage({ event: "finished", data: { workspaceId: sessionId, messageCount: 1 } });
    nextDispatcherActiveRunId(sessionId);
    resolveHistory([snapshot]);
    await Promise.resolve();
    expect(subscriber).not.toHaveBeenCalled();
  });
});

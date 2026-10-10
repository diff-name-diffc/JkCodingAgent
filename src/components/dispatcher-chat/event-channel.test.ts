import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { DispatcherMessageWire } from "../../types";
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
  it("assistantMessage 不清空或降级已显示正文与思考，下一阶段才改变阶段标记", () => {
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
    expect(getDispatcherLiveSessionState(sessionId)?.streamingSegments).toMatchObject([
      { messageId: "assistant-1", text: "先检查配置", superseded: true },
    ]);
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

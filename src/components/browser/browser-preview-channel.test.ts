import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { BrowserFrameEvent } from "../../types";

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));

const callbacks = new Map<string, (event: { payload: unknown }) => void>();
const unlisten = vi.fn();
let cleanup: Array<() => void> = [];

function frame(overrides: Partial<BrowserFrameEvent> = {}): BrowserFrameEvent {
  return {
    sessionId: "session-a",
    toolCallId: "call-a",
    runId: "run-a",
    streamId: "stream-a",
    sequence: 1,
    capturedAt: 1_728_728_000_000,
    url: "https://example.com",
    data: "data:image/jpeg;base64,AA",
    width: 1280,
    height: 800,
    ...overrides,
  };
}

beforeEach(() => {
  vi.resetModules();
  vi.useFakeTimers();
  callbacks.clear();
  unlisten.mockClear();
  mocks.invoke.mockReset().mockResolvedValue(undefined);
  mocks.listen.mockReset().mockImplementation((name, callback) => {
    callbacks.set(name, callback);
    return Promise.resolve(unlisten);
  });
});
afterEach(async () => {
  cleanup.forEach((stop) => stop());
  cleanup = [];
  await vi.advanceTimersByTimeAsync(0);
  vi.useRealTimers();
});

describe("共享浏览器预览事件通道", () => {
  it("多个窗口只建立一路帧监听，独立租约关闭，最后一个窗口卸载后释放监听", async () => {
    const { observeBrowserPreview } = await import("./browser-preview-channel");
    const onFrame = vi.fn();
    const observe = () =>
      observeBrowserPreview({
        scope: { sessionId: "session-a", toolCallId: "call-a" },
        onFrame,
        onError: vi.fn(),
      });
    const first = observe();
    const second = observe();
    await vi.advanceTimersByTimeAsync(0);
    expect(mocks.listen.mock.calls.map(([name]) => name)).toEqual([
      "browser-frame",
      "browser-preview-error",
    ]);
    expect(mocks.invoke).toHaveBeenCalledTimes(2);
    const ids = mocks.invoke.mock.calls.map(([, args]) => args.observerId);
    expect(new Set(ids).size).toBe(2);
    callbacks.get("browser-frame")!({ payload: frame() });
    expect(onFrame).toHaveBeenCalledTimes(2);
    first();
    await vi.advanceTimersByTimeAsync(3_000);
    expect(unlisten).not.toHaveBeenCalled();
    expect(
      mocks.invoke.mock.calls
        .filter(([, args]) => args.observerId === ids[0])
        .map(([, args]) => args.visible),
    ).toEqual([true, false]);
    second();
    expect(unlisten).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(0);
    expect(mocks.invoke).toHaveBeenLastCalledWith(
      "browser_preview_set",
      expect.objectContaining({ observerId: ids[1], visible: false }),
    );
  });

  it("事件连接成功前不注册；连接在卸载后才完成时也不重启采集", async () => {
    const resolve: Array<(stop: () => void) => void> = [];
    mocks.listen.mockImplementation(() => new Promise<() => void>((done) => resolve.push(done)));
    const { observeBrowserPreview } = await import("./browser-preview-channel");
    const stop = observeBrowserPreview({
      scope: { sessionId: "a", toolCallId: "b" },
      onFrame: vi.fn(),
      onError: vi.fn(),
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(mocks.invoke).not.toHaveBeenCalled();
    stop();
    resolve.forEach((done) => done(unlisten));
    await vi.advanceTimersByTimeAsync(0);
    expect(mocks.invoke).not.toHaveBeenCalled();
    expect(unlisten).toHaveBeenCalledTimes(2);
  });

  it("监听连接失败明确报错且不启动无人接收的采集", async () => {
    mocks.listen.mockRejectedValueOnce(new Error("webview已关闭"));
    const { observeBrowserPreview } = await import("./browser-preview-channel");
    const onError = vi.fn();
    cleanup.push(
      observeBrowserPreview({
        scope: { sessionId: "a", toolCallId: "b" },
        onFrame: vi.fn(),
        onError,
      }),
    );
    await vi.advanceTimersByTimeAsync(0);
    expect(onError).toHaveBeenCalledWith(expect.stringContaining("webview已关闭"));
    expect(mocks.invoke).not.toHaveBeenCalled();
  });

  it("帧与错误按会话/执行分发，冻结后迟到帧不更新旧轨迹", async () => {
    const {
      observeBrowserPreview,
      freezeBrowserPreview,
      getBrowserPreviewFrame,
      clearBrowserPreviewSession,
    } = await import("./browser-preview-channel");
    const scoped = {
      scope: { sessionId: "session-a", toolCallId: "call-a" },
      onFrame: vi.fn(),
      onError: vi.fn(),
    };
    const general = {
      scope: { sessionId: "session-a", toolCallId: null },
      onFrame: vi.fn(),
      onError: vi.fn(),
    };
    cleanup.push(observeBrowserPreview(scoped), observeBrowserPreview(general));
    await vi.advanceTimersByTimeAsync(0);
    const emit = (payload: BrowserFrameEvent) => callbacks.get("browser-frame")!({ payload });
    const first = frame();
    emit(frame({ sessionId: "session-b" }));
    expect(scoped.onFrame).not.toHaveBeenCalled();
    emit(first);
    freezeBrowserPreview(scoped.scope);
    emit(frame({ sequence: 2 }));
    emit(frame({ sequence: 3, toolCallId: "call-b", runId: "run-b" }));
    expect(scoped.onFrame).toHaveBeenCalledExactlyOnceWith(first);
    expect(general.onFrame).toHaveBeenCalledTimes(3);
    expect(getBrowserPreviewFrame(scoped.scope)).toBe(first);
    scoped.onError.mockClear();
    general.onError.mockClear();
    callbacks.get("browser-preview-error")!({
      payload: {
        sessionId: "session-a",
        toolCallId: "call-b",
        streamId: "stream-a",
        message: "截图超时",
      },
    });
    expect(scoped.onError).not.toHaveBeenCalled();
    expect(general.onError).toHaveBeenCalledExactlyOnceWith("截图超时");
    clearBrowserPreviewSession("session-a");
    expect(getBrowserPreviewFrame(scoped.scope)).toBe(null);
  });
});

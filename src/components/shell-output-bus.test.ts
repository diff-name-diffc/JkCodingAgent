import { describe, it, expect, beforeEach, vi } from "vitest";

/**
 * shell-output-bus.test.ts —— UI-24 遗留⑧：单一全局 shell-output 监听者，
 * 按 shell_id 分发到各终端的回调集合；退订清空则删键；重复订阅不重复注册全局监听。
 */

type ShellOutputHandler = (event: { payload: { shell_id: string; data: string } }) => void;

const mockListen = vi.fn();
let capturedHandler: ShellOutputHandler | null = null;

vi.mock("@tauri-apps/api/event", () => ({
  listen: (...args: unknown[]) => {
    capturedHandler = args[1] as ShellOutputHandler;
    return mockListen(...args);
  },
}));

async function loadBus() {
  // 重置模块单例态（listenerRegistered / subscribers Map）。
  vi.resetModules();
  return await import("./shell-output-bus");
}

beforeEach(() => {
  vi.clearAllMocks();
  capturedHandler = null;
  mockListen.mockResolvedValue(() => {});
});

describe("subscribeShellOutput", () => {
  it("懒注册：首次订阅才注册全局监听，订阅前无监听者", async () => {
    const { subscribeShellOutput } = await loadBus();
    expect(capturedHandler).toBeNull();
    expect(mockListen).not.toHaveBeenCalled();

    subscribeShellOutput("shell-a", () => {});
    expect(mockListen).toHaveBeenCalledTimes(1);
    expect(mockListen.mock.calls[0][0]).toBe("shell-output");
    expect(capturedHandler).not.toBeNull();
  });

  it("重复订阅不重复注册全局监听", async () => {
    const { subscribeShellOutput } = await loadBus();
    subscribeShellOutput("shell-a", () => {});
    subscribeShellOutput("shell-b", () => {});
    subscribeShellOutput("shell-a", () => {});
    expect(mockListen).toHaveBeenCalledTimes(1);
  });

  it("按 shell_id 分发：只投递给对应终端的回调，且不串台", async () => {
    const { subscribeShellOutput } = await loadBus();
    const a = vi.fn();
    const b = vi.fn();
    subscribeShellOutput("shell-a", a);
    subscribeShellOutput("shell-b", b);

    capturedHandler!({ payload: { shell_id: "shell-a", data: "hello" } });
    expect(a).toHaveBeenCalledWith("hello");
    expect(b).not.toHaveBeenCalled();

    capturedHandler!({ payload: { shell_id: "shell-b", data: "world" } });
    expect(b).toHaveBeenCalledWith("world");
    expect(a).toHaveBeenCalledTimes(1);
  });

  it("同一终端多个回调都被分发", async () => {
    const { subscribeShellOutput } = await loadBus();
    const a = vi.fn();
    const b = vi.fn();
    subscribeShellOutput("shell-a", a);
    subscribeShellOutput("shell-a", b);

    capturedHandler!({ payload: { shell_id: "shell-a", data: "x" } });
    expect(a).toHaveBeenCalledWith("x");
    expect(b).toHaveBeenCalledWith("x");
  });

  it("退订后不再接收；集合清空则删除键（后续事件不分发）", async () => {
    const { subscribeShellOutput } = await loadBus();
    const a = vi.fn();
    const unsubscribe = subscribeShellOutput("shell-a", a);
    unsubscribe();

    capturedHandler!({ payload: { shell_id: "shell-a", data: "x" } });
    expect(a).not.toHaveBeenCalled();
  });

  it("只退订一个回调不影响同终端其它回调", async () => {
    const { subscribeShellOutput } = await loadBus();
    const a = vi.fn();
    const b = vi.fn();
    subscribeShellOutput("shell-a", a);
    const unsubB = subscribeShellOutput("shell-a", b);
    unsubB();

    capturedHandler!({ payload: { shell_id: "shell-a", data: "x" } });
    expect(a).toHaveBeenCalledWith("x");
    expect(b).not.toHaveBeenCalled();
  });

  it("回调内退订不破坏当前分发（快照迭代）", async () => {
    const { subscribeShellOutput } = await loadBus();
    const a = vi.fn();
    const b = vi.fn();
    const unsubA = subscribeShellOutput("shell-a", a);
    const bThatUnsubsA = (data: string) => {
      unsubA();
      b(data);
    };
    subscribeShellOutput("shell-a", bThatUnsubsA);

    capturedHandler!({ payload: { shell_id: "shell-a", data: "x" } });
    // a 在 b 之前注册，当前帧仍被分发；b 执行后退订 a。
    expect(a).toHaveBeenCalledWith("x");
    expect(b).toHaveBeenCalledWith("x");
  });
});

import type { ComponentProps } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { BrowserFrameEvent } from "../../types";
import { BrowserAgentMonitor } from "./BrowserAgentMonitor";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  preview: vi.fn(),
  stop: undefined as (() => void) | undefined,
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("./useBrowserPreview", () => ({ useBrowserPreview: mocks.preview }));
vi.mock("../ui/button", () => ({
  Button: ({
    variant: _variant,
    size: _size,
    ...props
  }: ComponentProps<"button"> & { variant?: string; size?: string }) => {
    mocks.stop = props.onClick as (() => void) | undefined;
    return <button {...props} />;
  },
}));

const frame: BrowserFrameEvent = {
  sessionId: "dispatcher-a",
  runId: "run-a",
  toolCallId: "call-a",
  streamId: "stream-a",
  sequence: 3,
  capturedAt: 1_728_728_000_000,
  url: "https://example.com/search?q=browser",
  data: "data:image/jpeg;base64,not-for-react-state",
  width: 1280,
  height: 800,
};

function render(status: ComponentProps<typeof BrowserAgentMonitor>["status"] = "running") {
  return renderToStaticMarkup(
    <BrowserAgentMonitor sessionId="dispatcher-a" toolCallId="call-a" status={status} />,
  );
}

beforeEach(() => {
  mocks.stop = undefined;
  mocks.invoke.mockClear();
  mocks.preview.mockReset().mockReturnValue({
    canvasRef: { current: null },
    frame,
    error: null,
    visible: true,
  });
});

describe("浏览器子智能体监控", () => {
  it("按父会话和工具调用身份观察当前执行，画面始终使用只读 Canvas", () => {
    const html = render();
    expect(mocks.preview).toHaveBeenCalledWith(
      expect.objectContaining({
        sessionId: "dispatcher-a",
        toolCallId: "call-a",
        enabled: true,
        finished: false,
      }),
    );
    expect(html).toContain("只读监控");
    expect(html).toContain("实时监控");
    expect(html).toContain(frame.url);
    expect(html).toContain(new Date(frame.capturedAt).toISOString());
    expect(html).toContain("<canvas");
    expect(html).not.toContain("<iframe");
    expect(html).not.toContain(frame.data);
  });

  it("停止只提交本次执行身份，重复点击不重复发请求", async () => {
    render();
    expect(mocks.stop).toBeDefined();
    mocks.stop!();
    mocks.stop!();
    await Promise.resolve();
    expect(mocks.invoke).toHaveBeenCalledExactlyOnceWith("browser_cancel_exploration", {
      sessionId: "dispatcher-a",
      toolCallId: "call-a",
    });
  });

  it("停止请求失败后允许重试，仍绑定同一次执行", async () => {
    mocks.invoke.mockRejectedValueOnce(new Error("执行仍在启动，暂时无法确认停止"));
    render();
    mocks.stop!();
    await Promise.resolve();
    mocks.stop!();
    await Promise.resolve();
    expect(mocks.invoke).toHaveBeenCalledTimes(2);
    expect(mocks.invoke).toHaveBeenLastCalledWith("browser_cancel_exploration", {
      sessionId: "dispatcher-a",
      toolCallId: "call-a",
    });
  });

  it.each([
    ["completed", "执行已完成"],
    ["failed", "执行已失败"],
    ["cancelled", "执行已取消"],
    ["timed_out", "执行已超时"],
  ] as const)("%s 终态冻结最后画面且不允许再次停止", (status, label) => {
    const html = render(status);
    expect(html).toContain(`${label} · 最后画面`);
    expect(html).not.toContain("停止本次浏览器探索");
    expect(mocks.preview).toHaveBeenCalledWith(
      expect.objectContaining({ enabled: false, finished: true }),
    );
    expect(mocks.invoke).not.toHaveBeenCalled();
  });

  it("无采集记录的历史执行明确显示空态", () => {
    mocks.preview.mockReturnValue({ canvasRef: { current: null }, frame: null, error: null });
    const html = render("completed");
    expect(html).toContain("本次执行暂无可用画面");
    expect(html).not.toContain("最后画面");
    expect(html).toContain("<canvas");
  });

  it("暂停观察与采集错误明确展示，不冒充执行已停止", () => {
    mocks.preview.mockReturnValue({
      canvasRef: { current: null },
      frame,
      error: "截图采集超时",
      visible: false,
    });
    const html = render();
    expect(html).toContain("监控已暂停");
    expect(html).toContain("画面监控失败：截图采集超时");
    expect(html).not.toContain("执行已取消");
    expect(html).toContain("停止本次浏览器探索");
  });
});

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PreviewLease } from "./preview-lease";

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("可见预览租约", () => {
  it("仅可见时立即注册并每三秒续租，隐藏后发送关闭且停止续租", async () => {
    const send = vi.fn().mockResolvedValue(undefined);
    const lease = new PreviewLease(send, vi.fn());
    await vi.advanceTimersByTimeAsync(9_000);
    expect(send).not.toHaveBeenCalled();
    lease.setVisible(true);
    await vi.advanceTimersByTimeAsync(6_000);
    expect(send.mock.calls).toEqual([[true], [true], [true]]);
    lease.setVisible(false);
    await vi.advanceTimersByTimeAsync(9_000);
    expect(send.mock.calls).toEqual([[true], [true], [true], [false]]);
    lease.dispose();
  });

  it("注册在途时卸载，最后发送关闭；积压续租不会复活已隐藏窗口", async () => {
    let resolve!: () => void;
    const pending = new Promise<void>((done) => {
      resolve = done;
    });
    const send = vi
      .fn()
      .mockImplementationOnce(() => pending)
      .mockResolvedValue(undefined);
    const lease = new PreviewLease(send, vi.fn());
    lease.setVisible(true);
    await vi.advanceTimersByTimeAsync(12_000);
    expect(send.mock.calls).toEqual([[true]]);
    lease.dispose();
    resolve();
    await vi.advanceTimersByTimeAsync(12_000);
    expect(send.mock.calls).toEqual([[true], [false]]);
    lease.setVisible(true);
    expect(send).toHaveBeenCalledTimes(2);
  });

  it("合并慢请求期间的多次切换，且不并发发送租约", async () => {
    let resolve!: () => void;
    const pending = new Promise<void>((done) => {
      resolve = done;
    });
    const send = vi
      .fn()
      .mockImplementationOnce(() => pending)
      .mockResolvedValue(undefined);
    const lease = new PreviewLease(send, vi.fn());
    lease.setVisible(true);
    lease.setVisible(false);
    lease.setVisible(true);
    expect(send.mock.calls).toEqual([[true]]);
    resolve();
    await vi.advanceTimersByTimeAsync(0);
    expect(send.mock.calls).toEqual([[true], [true]]);
    lease.dispose();
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenLastCalledWith(false);
  });

  it("注册错误明确报告，下次续租可恢复", async () => {
    const send = vi.fn().mockRejectedValueOnce(new Error("连接断开")).mockResolvedValue(undefined);
    const onError = vi.fn();
    const lease = new PreviewLease(send, onError);
    lease.setVisible(true);
    await vi.advanceTimersByTimeAsync(0);
    expect(onError).toHaveBeenLastCalledWith(expect.stringContaining("连接断开"));
    await vi.advanceTimersByTimeAsync(3_000);
    expect(onError).toHaveBeenLastCalledWith(null);
    lease.dispose();
    await vi.advanceTimersByTimeAsync(0);
  });
});

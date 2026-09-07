import { describe, expect, it } from "vitest";
import { createRequestGuard } from "./request-guard";

describe("createRequestGuard", () => {
  it("begin 返回自增代号（从 2 起——1 为初始代号，0 为未领取哨兵）", () => {
    const guard = createRequestGuard();
    expect(guard.begin()).toBe(2);
    expect(guard.begin()).toBe(3);
    expect(guard.begin()).toBe(4);
  });

  it("isStale：旧代号过期，最新代号有效", () => {
    const guard = createRequestGuard();
    const first = guard.begin();
    expect(guard.isStale(first)).toBe(false);
    const second = guard.begin();
    expect(guard.isStale(first)).toBe(true);
    expect(guard.isStale(second)).toBe(false);
  });

  it("invalidate 作废当前代号而不产生新请求", () => {
    const guard = createRequestGuard();
    const id = guard.begin();
    guard.invalidate();
    expect(guard.isStale(id)).toBe(true);
    // 下一次 begin 继续自增，不复用作废前的代号
    expect(guard.begin()).toBe(id + 2);
  });

  it("初始状态：未领取的哨兵代号 0 恒过期（调用方以 begin 返回值为判据）", () => {
    const guard = createRequestGuard();
    // 0 = 未领取哨兵：任何时刻都不等于真实代号（begin 从 2 起），恒判过期。
    expect(guard.isStale(0)).toBe(true);
    // 1 = 初始代号（等价原 hook 挂载后状态），未被 begin 领取；
    // 消费方不会以它调用 isStale，不做断言。
  });

  it("多实例互不干扰", () => {
    const a = createRequestGuard();
    const b = createRequestGuard();
    const aId = a.begin();
    b.begin();
    b.begin();
    expect(a.isStale(aId)).toBe(false);
    expect(b.isStale(aId)).toBe(true);
  });
});

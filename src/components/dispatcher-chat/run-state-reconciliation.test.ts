import { describe, expect, it } from "vitest";
import { planDetachedRuns } from "./run-state-reconciliation";

describe("planDetachedRuns", () => {
  it("收养后端仍在跑且无事件通道的会话", () => {
    const plan = planDetachedRuns(
      new Set(["ws-1", "ws-2"]),
      new Set<string>(),
      () => false,
    );
    expect(plan.adopt).toEqual(["ws-1", "ws-2"]);
    expect(plan.release).toEqual([]);
  });

  it("跳过已有本地事件通道的会话——其状态由 Channel 自行驱动", () => {
    const plan = planDetachedRuns(
      new Set(["ws-live", "ws-detached"]),
      new Set<string>(),
      (sessionId) => sessionId === "ws-live",
    );
    expect(plan.adopt).toEqual(["ws-detached"]);
  });

  it("跳过已在托管中的会话（不重复收养）", () => {
    const plan = planDetachedRuns(
      new Set(["ws-1"]),
      new Set(["ws-1"]),
      () => false,
    );
    expect(plan.adopt).toEqual([]);
    expect(plan.release).toEqual([]);
  });

  it("后端收尾的托管会话进入释放清单", () => {
    const plan = planDetachedRuns(
      new Set<string>(),
      new Set(["ws-1", "ws-2"]),
      () => false,
    );
    expect(plan.release).toEqual(["ws-1", "ws-2"]);
  });

  it("仍在后端运行清单中的托管会话既不收养也不释放", () => {
    const plan = planDetachedRuns(
      new Set(["ws-1"]),
      new Set(["ws-1", "ws-finished"]),
      () => false,
    );
    expect(plan.adopt).toEqual([]);
    expect(plan.release).toEqual(["ws-finished"]);
  });
});

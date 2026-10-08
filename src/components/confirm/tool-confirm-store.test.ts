import { afterEach, describe, expect, it, vi } from "vitest";
import {
  enqueueToolConfirm,
  getPendingToolConfirms,
  removeToolConfirm,
  resetToolConfirms,
  subscribeToolConfirms,
  type ToolConfirmRequest,
} from "./tool-confirm-store";

function request(id: string, overrides: Partial<ToolConfirmRequest> = {}): ToolConfirmRequest {
  return {
    requestId: id,
    workspaceId: "ws-1",
    sessionId: "ws-1",
    tool: "ssh_exec",
    target: "SSH server prod",
    command: "systemctl restart nginx",
    reason: "涉及生产服务重启",
    elevated: false,
    ...overrides,
  };
}

afterEach(() => {
  resetToolConfirms();
});

describe("tool-confirm-store", () => {
  it("enqueues requests in arrival order", () => {
    enqueueToolConfirm(request("a"));
    enqueueToolConfirm(request("b"));
    expect(getPendingToolConfirms().map((r) => r.requestId)).toEqual(["a", "b"]);
  });

  it("replaces same requestId instead of duplicating", () => {
    enqueueToolConfirm(request("a", { reason: "旧" }));
    enqueueToolConfirm(request("a", { reason: "新" }));
    const pending = getPendingToolConfirms();
    expect(pending).toHaveLength(1);
    expect(pending[0].reason).toBe("新");
  });

  it("removes only the targeted request", () => {
    enqueueToolConfirm(request("a"));
    enqueueToolConfirm(request("b"));
    removeToolConfirm("a");
    expect(getPendingToolConfirms().map((r) => r.requestId)).toEqual(["b"]);
  });

  it("notifies subscribers on enqueue and removal", () => {
    const listener = vi.fn();
    const unsubscribe = subscribeToolConfirms(listener);
    enqueueToolConfirm(request("a"));
    expect(listener).toHaveBeenCalledTimes(1);
    removeToolConfirm("a");
    expect(listener).toHaveBeenCalledTimes(2);
    // 不存在的 id 移除不触发通知（避免空转重渲染）。
    removeToolConfirm("missing");
    expect(listener).toHaveBeenCalledTimes(2);
    unsubscribe();
  });
});

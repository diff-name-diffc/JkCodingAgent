import { describe, expect, it } from "vitest";
import { NODE_STATUS_META, PLAN_STATUS_META } from "../graph/graph-utils";
import { resolveStatusMeta } from "./status-meta";

describe("resolveStatusMeta", () => {
  it("tool 域四态全覆盖（planned 独立于 running）", () => {
    expect(resolveStatusMeta("tool", "planned")).toEqual({ tone: "pending", label: "等待" });
    expect(resolveStatusMeta("tool", "running")).toEqual({ tone: "running", label: "执行中" });
    expect(resolveStatusMeta("tool", "success")).toEqual({ tone: "success", label: "成功" });
    expect(resolveStatusMeta("tool", "error")).toEqual({ tone: "error", label: "失败" });
  });

  it("graph-node 域与 NODE_STATUS_META 标签一致", () => {
    for (const status of ["pending", "running", "succeeded", "failed", "skipped", "cancelled"]) {
      expect(resolveStatusMeta("graph-node", status).label).toBe(
        NODE_STATUS_META[status as keyof typeof NODE_STATUS_META].label,
      );
    }
    expect(resolveStatusMeta("graph-node", "failed").tone).toBe("error");
    expect(resolveStatusMeta("graph-node", "pending").tone).toBe("pending");
  });

  it("graph-plan 域与 PLAN_STATUS_META 标签一致", () => {
    for (const status of ["draft", "running", "completed", "failed", "cancelled"]) {
      expect(resolveStatusMeta("graph-plan", status).label).toBe(
        PLAN_STATUS_META[status as keyof typeof PLAN_STATUS_META].label,
      );
    }
    expect(resolveStatusMeta("graph-plan", "cancelled").tone).toBe("warn");
  });

  it("python 域含 stopped/idle，failed 为 error tone", () => {
    expect(resolveStatusMeta("python", "running").tone).toBe("running");
    expect(resolveStatusMeta("python", "done")).toEqual({ tone: "success", label: "已完成" });
    expect(resolveStatusMeta("python", "failed")).toEqual({ tone: "error", label: "执行失败" });
    expect(resolveStatusMeta("python", "stopped")).toEqual({ tone: "warn", label: "已停止" });
    expect(resolveStatusMeta("python", "idle")).toEqual({ tone: "neutral", label: "未运行" });
  });

  it("subagent 与 verdict 域全覆盖", () => {
    expect(resolveStatusMeta("subagent", "completed").tone).toBe("success");
    expect(resolveStatusMeta("subagent", "failed").tone).toBe("error");
    expect(resolveStatusMeta("verdict", "pass")).toEqual({ tone: "success", label: "验收通过" });
    expect(resolveStatusMeta("verdict", "partial").tone).toBe("warn");
    expect(resolveStatusMeta("verdict", "fail").tone).toBe("error");
    expect(resolveStatusMeta("verdict", "unknown")).toEqual({
      tone: "neutral",
      label: "未能验收",
    });
  });

  it("未知状态回退 neutral + 原文（不虚构成功）", () => {
    expect(resolveStatusMeta("tool", "weird-state")).toEqual({
      tone: "neutral",
      label: "weird-state",
    });
    expect(resolveStatusMeta("python", "")).toEqual({ tone: "neutral", label: "" });
  });
});

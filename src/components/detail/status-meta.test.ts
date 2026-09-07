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

  it("connection 域（UI-22b）：degraded 与 invalid_config 区分不压平", () => {
    expect(resolveStatusMeta("connection", "checking")).toEqual({ tone: "running", label: "检查中" });
    expect(resolveStatusMeta("connection", "not_configured")).toEqual({
      tone: "neutral",
      label: "未配置",
    });
    expect(resolveStatusMeta("connection", "healthy")).toEqual({ tone: "success", label: "正常" });
    expect(resolveStatusMeta("connection", "degraded")).toEqual({ tone: "error", label: "异常" });
    expect(resolveStatusMeta("connection", "invalid_config")).toEqual({
      tone: "error",
      label: "配置无效",
    });
  });

  it("mcp-server 域（UI-22b）：五态双编码，spawn_failed 为 warn 可重试", () => {
    expect(resolveStatusMeta("mcp-server", "disabled")).toEqual({ tone: "neutral", label: "已禁用" });
    expect(resolveStatusMeta("mcp-server", "healthy")).toEqual({ tone: "success", label: "正常" });
    expect(resolveStatusMeta("mcp-server", "invalid_config")).toEqual({
      tone: "error",
      label: "配置无效",
    });
    expect(resolveStatusMeta("mcp-server", "spawn_failed")).toEqual({
      tone: "warn",
      label: "启动失败",
    });
    expect(resolveStatusMeta("mcp-server", "connection_failed")).toEqual({
      tone: "error",
      label: "连接失败",
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

import { describe, expect, it } from "vitest";
import { applySubAgentEvent, getSubAgentSession, hydrateSubAgentTrace } from "./subAgentEventStore";
import type { SubAgentEvent } from "../types";

/**
 * UI-14 遗留：子智能体轨迹真实 model 字段的回放归一化。
 * 双通道语义——Started 事件为实时源；trace 表 model 列为回放权威源
 * （长任务中 Started 会被容量裁剪逐出）；两源皆无 → undefined，
 * 视图显示「未记录」兜底。
 */
function startedEvent(data: SubAgentEvent["data"]): SubAgentEvent {
  return { event: "Started", data };
}

describe("hydrateSubAgentTrace model 归一化", () => {
  it("Started 事件携带 model 时回放写入 session.model", () => {
    const session = hydrateSubAgentTrace("s-model-1", "tc-1", [
      startedEvent({ agentId: "a", agentName: "A", task: "t", model: "m" }),
    ]);
    expect(session).not.toBeNull();
    expect(session!.model).toBe("m");
  });

  it("trace 列值权威覆盖：Started 被裁剪逐出（无 model）时用列值", () => {
    // 模拟长任务裁剪后事件流只剩尾部事件、Started 缺失 model 的情形。
    const session = hydrateSubAgentTrace(
      "s-model-2",
      "tc-2",
      [startedEvent({ agentId: "a", agentName: "A", task: "t" })],
      "col-model",
    );
    expect(session!.model).toBe("col-model");
  });

  it("列值优先于事件值（列为回放权威源）", () => {
    const session = hydrateSubAgentTrace(
      "s-model-3",
      "tc-3",
      [startedEvent({ agentId: "a", agentName: "A", task: "t", model: "event-model" })],
      "col-model",
    );
    expect(session!.model).toBe("col-model");
  });

  it("老轨迹（事件无 model、列为 null）→ undefined，视图「未记录」兜底", () => {
    const session = hydrateSubAgentTrace(
      "s-model-4",
      "tc-4",
      [startedEvent({ agentId: "a", agentName: "A", task: "t" })],
      null,
    );
    expect(session).not.toBeNull();
    expect(session!.model).toBeUndefined();
  });
});

describe("browser-use 执行轨迹", () => {
  const agentId = "browser-agent";

  it("运行中的 checkpoint 回放仍保持运行状态", () => {
    const session = hydrateSubAgentTrace("browser-checkpoint", "call-browser", [
      startedEvent({ agentId, task: "读取网页" }),
      { event: "ToolStarted", data: { agentId, taskId: "step-1-action-0", toolName: "navigate" } },
    ]);
    expect(session?.status).toBe("running");
    expect(session?.toolCalls[0].status).toBe("running");
  });

  it("原生动作按 taskId 配对，失败动作保留错误并允许随后重试", () => {
    const session = hydrateSubAgentTrace("browser-actions", "call-browser", [
      startedEvent({ agentId, agentName: "浏览器探索", task: "查找页面", model: "model" }),
      {
        event: "ToolStarted",
        data: { agentId, taskId: "step-1-action-0", toolName: "click", arguments: { index: 1 } },
      },
      {
        event: "ToolFinished",
        data: { agentId, taskId: "step-1-action-0", toolName: "click", error: "目标已变化" },
      },
      { event: "Progress", data: { agentId, message: "正在重新观察页面" } },
      {
        event: "ToolStarted",
        data: { agentId, taskId: "step-2-action-0", toolName: "click", arguments: { index: 2 } },
      },
      {
        event: "ToolFinished",
        data: {
          agentId,
          taskId: "step-2-action-0",
          toolName: "click",
          resultPreview: "已打开详情",
        },
      },
      {
        event: "Finished",
        data: {
          agentId,
          result: "已核实页面",
          iterations: 2,
          elapsedMs: 1000,
          tokenUsage: { promptTokens: 10, completionTokens: 5, totalTokens: 15 },
        },
      },
    ]);
    expect(session?.status).toBe("completed");
    expect(
      session?.toolCalls.map(({ id, toolName, status, resultPreview }) => ({
        id,
        toolName,
        status,
        resultPreview,
      })),
    ).toEqual([
      { id: "step-1-action-0", toolName: "click", status: "failed", resultPreview: "目标已变化" },
      {
        id: "step-2-action-0",
        toolName: "click",
        status: "completed",
        resultPreview: "已打开详情",
      },
    ]);
    expect(session?.finishedResult).toBe("已核实页面");
    expect(session?.tokenUsage?.totalTokens).toBe(15);
  });

  it.each(["cancelled", "timed_out"] as const)(
    "%s 保留真实终态和用量，结束未完成动作",
    (terminationReason) => {
      const session = hydrateSubAgentTrace(`browser-${terminationReason}`, "call-browser", [
        startedEvent({ agentId, task: "等待页面" }),
        {
          event: "UsageUpdated",
          data: { agentId, tokenUsage: { promptTokens: 8, completionTokens: 2, totalTokens: 10 } },
        },
        {
          event: "ToolStarted",
          data: { agentId, taskId: "step-1-action-0", toolName: "navigate" },
        },
        { event: "Failed", data: { agentId, terminationReason, error: "浏览器任务已停止" } },
      ]);
      expect(session?.status).toBe(terminationReason);
      expect(session?.phase).toBe(terminationReason);
      expect(session?.tokenUsage?.totalTokens).toBe(10);
      expect(session?.toolCalls[0].status).toBe("failed");
      expect(session?.toolCalls[0].resultPreview).toBe("浏览器任务已停止");
      expect(session?.finishedError).toBe("浏览器任务已停止");
    },
  );
});

describe("checkpoint 与实时事件竞态", () => {
  it("长任务裁剪标记不覆盖已恢复的 Agent 身份，并明确展示裁剪提示", () => {
    const session = hydrateSubAgentTrace("truncated-running", "call", [
      { event: "traceTruncated", timestampMs: 130, data: { dropped: 20 } },
      {
        event: "Progress",
        timestampMs: 120,
        data: { agentId: "browser-agent", agentName: "浏览器", message: "继续读取" },
      },
    ])!;
    expect(session.agentId).toBe("browser-agent");
    expect(session.name).toBe("浏览器");
    expect(session.status).toBe("running");
    expect(session.events[session.events.length - 1]?.text).toContain("20 条轨迹事件已被裁剪");
  });

  it("终态快照从裁剪标记开始时仍能恢复后续事件的 Agent 名称", () => {
    const session = hydrateSubAgentTrace("truncated-finished", "call", [
      { event: "traceTruncated", timestampMs: 130, data: { dropped: 20 } },
      {
        event: "Failed",
        timestampMs: 130,
        data: { agentId: "browser-agent", agentName: "浏览器", error: "已停止" },
      },
    ])!;
    expect(session.agentId).toBe("browser-agent");
    expect(session.name).toBe("浏览器");
    expect(session.status).toBe("failed");
  });

  const started: SubAgentEvent = {
    event: "Started",
    timestampMs: 100,
    data: { agentId: "browser-agent", agentName: "浏览器助手", task: "搜索资料" },
  };
  const action: SubAgentEvent = {
    event: "ToolStarted",
    timestampMs: 110,
    data: {
      agentId: "browser-agent",
      taskId: "action-1",
      toolName: "navigate",
      arguments: { url: "https://example.test" },
    },
  };

  function live(sessionId: string, event: SubAgentEvent) {
    applySubAgentEvent({
      ...event,
      sessionId,
      toolCallId: "call",
      timestampMs: event.timestampMs ?? 110,
    });
  }

  it("读取旧 checkpoint 期间收到同毫秒终态，回放后仍完成且补全历史动作", () => {
    const sessionId = "same-ms-complete";
    live(sessionId, {
      event: "ToolFinished",
      timestampMs: 110,
      data: {
        agentId: "browser-agent",
        taskId: "action-1",
        toolName: "navigate",
        resultPreview: "loaded",
      },
    });
    live(sessionId, {
      event: "Finished",
      timestampMs: 110,
      data: {
        agentId: "browser-agent",
        result: "完成",
        elapsedMs: 10,
        tokenUsage: { promptTokens: 2, completionTokens: 1, totalTokens: 3 },
      },
    });
    const result = hydrateSubAgentTrace(sessionId, "call", [started, action]);
    expect(result?.status).toBe("completed");
    expect(result?.task).toBe("搜索资料");
    expect(result?.toolCalls[0].status).toBe("completed");
    expect(result?.finishedResult).toBe("完成");
    expect(result?.tokenUsage?.totalTokens).toBe(3);
  });

  it.each(["cancelled", "timed_out"] as const)(
    "旧 checkpoint 不覆盖实时 %s",
    (terminationReason) => {
      const sessionId = `same-ms-${terminationReason}`;
      live(sessionId, {
        event: "Failed",
        timestampMs: 110,
        data: { agentId: "browser-agent", terminationReason, error: "停止" },
      });
      const result = hydrateSubAgentTrace(sessionId, "call", [started, action]);
      expect(result?.status).toBe(terminationReason);
      expect(result?.toolCalls[0].status).toBe("failed");
    },
  );

  it("终态后迟到的动作不会重新进入运行阶段", () => {
    const sessionId = "late-action";
    live(sessionId, started);
    live(sessionId, {
      event: "Failed",
      timestampMs: 120,
      data: { agentId: "browser-agent", terminationReason: "cancelled", error: "停止" },
    });
    live(sessionId, action);
    expect(getSubAgentSession(sessionId, "call")?.phase).toBe("cancelled");
    expect(getSubAgentSession(sessionId, "call")?.toolCalls).toHaveLength(0);
  });

  it("真实旧 Rust wire 用量在 Failed 回放后仍可展示", () => {
    const events: SubAgentEvent[] = JSON.parse(
      '[{"event":"Started","timestampMs":100,"data":{"agentId":"browser-agent","agentName":"浏览器","task":"搜索","model":"m"}},{"event":"UsageUpdated","timestampMs":120,"data":{"agentId":"browser-agent","tokenUsage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10},"elapsedMs":20}},{"event":"Failed","timestampMs":130,"data":{"agentId":"browser-agent","error":"停止","terminationReason":"cancelled"}}]',
    );
    expect(hydrateSubAgentTrace("legacy-wire", "call", events)?.tokenUsage).toEqual({
      promptTokens: 7,
      completionTokens: 3,
      totalTokens: 10,
    });
  });
});

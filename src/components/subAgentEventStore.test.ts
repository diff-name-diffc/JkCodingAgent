import { describe, expect, it } from "vitest";
import { hydrateSubAgentTrace } from "./subAgentEventStore";
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

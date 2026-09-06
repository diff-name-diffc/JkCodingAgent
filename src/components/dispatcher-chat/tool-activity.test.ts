import { describe, expect, it } from "vitest";

import type { DispatcherToolRunRecord } from "../../types";
import { mergeToolRunRecords, toolRunStatusToCallStatus } from "./tool-activity";
import {
  finishLiveToolActivity,
  planLiveToolActivity,
  startLiveToolActivity,
} from "./live-tool-activity";

function run(patch: Partial<DispatcherToolRunRecord>): DispatcherToolRunRecord {
  return {
    id: "root",
    workspaceId: "workspace-1",
    toolCallId: "call-1",
    parentRunId: null,
    origin: "model",
    stepId: null,
    sequence: 0,
    toolName: "run_tool_program",
    provider: "builtin",
    category: "other",
    status: "running",
    argumentsJson: "{}",
    effectiveArgumentsJson: "{}",
    resultMode: null,
    messageId: null,
    errorKind: null,
    errorMessage: null,
    actionKind: null,
    startedAt: "2026-08-18T00:00:00Z",
    finishedAt: null,
    durationMs: 0,
    metadataJson: "{}",
    createdAt: "2026-08-18T00:00:00Z",
    updatedAt: "2026-08-18T00:00:00Z",
    ...patch,
  };
}

describe("mergeToolRunRecords", () => {
  it("用最新快照覆盖同一 run，并按父子与 sequence 稳定排序", () => {
    const root = run({});
    const second = run({
      id: "step-2",
      parentRunId: root.id,
      origin: "tool_program",
      stepId: "second",
      sequence: 2,
      toolName: "grep",
    });
    const first = run({
      id: "step-1",
      parentRunId: root.id,
      origin: "tool_program",
      stepId: "first",
      sequence: 1,
      toolName: "glob",
    });

    const merged = mergeToolRunRecords([root, second], [first, { ...second, status: "succeeded" }]);

    expect(merged.map((item) => item.id)).toEqual(["root", "step-1", "step-2"]);
    expect(merged.find((item) => item.id === "step-2")?.status).toBe("succeeded");
  });

  it("损坏的环不会导致无限递归，也不会丢记录", () => {
    const left = run({ id: "left", parentRunId: "right" });
    const right = run({ id: "right", parentRunId: "left" });

    expect(
      mergeToolRunRecords([], [left, right])
        .map((item) => item.id)
        .sort(),
    ).toEqual(["left", "right"]);
  });
});

describe("toolRunStatusToCallStatus", () => {
  it("保留运行态，并统一成功与错误终态", () => {
    expect(toolRunStatusToCallStatus("planned")).toBe("running");
    expect(toolRunStatusToCallStatus("succeeded")).toBe("success");
    expect(toolRunStatusToCallStatus("internal_error")).toBe("error");
    expect(toolRunStatusToCallStatus("cancelled")).toBe("error");
  });
});

describe("planned 标记全链路（UI-12：等待独立于执行中）", () => {
  const payload = { toolCallId: "call-1", name: "read_file", arguments: "{}" };

  it("planLive 置 planned=true，startLive 翻转为 false", () => {
    const planned = planLiveToolActivity([], payload);
    expect(planned[0].planned).toBe(true);
    expect(planned[0].status).toBe("running");

    const started = startLiveToolActivity(planned, payload);
    expect(started[0].planned).toBe(false);
    expect(started[0].status).toBe("running");
  });

  it("finishLive 清除 planned 并落终态", () => {
    const planned = planLiveToolActivity([], payload);
    const finished = finishLiveToolActivity(planned, {
      ...payload,
      displayText: "ok",
      contextPayload: "文件内容",
      resultMode: "raw",
      detailRefs: [],
    });
    expect(finished[0].planned).toBe(false);
    expect(finished[0].status).toBe("success");
  });

  it("跳过 started 直接 finish（run 切换兜底）不带 planned 标记", () => {
    const finished = finishLiveToolActivity([], {
      ...payload,
      displayText: "boom",
      contextPayload: "错误：读取失败",
      resultMode: "raw",
      detailRefs: [],
    });
    expect(finished[0].planned).toBeFalsy();
    expect(finished[0].status).toBe("error");
  });
});

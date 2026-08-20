import { describe, expect, it } from "vitest";

import type { DispatcherToolRunRecord } from "../../types";
import { mergeToolRunRecords, toolRunStatusToCallStatus } from "./tool-activity";

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

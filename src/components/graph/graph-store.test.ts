import { describe, expect, it } from "vitest";

import type {
  AgentActivity,
  GraphNodeRunRecord,
  GraphPlanRecord,
  GraphRunEventPayload,
} from "../../types";
import { createSerialTaskQueue, reduceGraphRunEvent, type GraphPlanSnapshot } from "./graph-store";

function nodeRun(): GraphNodeRunRecord {
  return {
    runId: "run-1",
    planId: "plan-1",
    nodeId: "node-1",
    status: "running",
    phase: "responding",
    modelRef: "model-1",
    modelLabel: "Model 1",
    modelCategory: "text",
    baseToolGroup: "coding",
    specialToolsJson: "[]",
    inputText: "input",
    outputText: "",
    errorText: null,
    startedAt: 1,
    finishedAt: null,
    durationMs: null,
    affectedFiles: [],
    usageJson: "{}",
    toolCallCount: 0,
    retryCount: 0,
  };
}

function plan(): GraphPlanRecord {
  return {
    id: "plan-1",
    workspaceId: "workspace-1",
    title: "Plan",
    summary: "",
    definitionJson: '{"version":3,"title":"Plan","stateKeys":[],"nodes":[]}',
    status: "running",
    stateJson: "{}",
    requirement: "需求",
    inheritsPlanId: null,
    inheritsRunId: null,
    createdAt: 1,
    updatedAt: 1,
    latestRunId: "run-1",
    runs: [],
    nodeRuns: [nodeRun()],
  };
}

function snapshot(): GraphPlanSnapshot {
  return { plan: plan(), liveOutputs: {}, liveActivities: {}, lastEvent: null, paused: false, pausedNodeId: null };
}

function payload(
  event: GraphRunEventPayload["event"],
  data: GraphRunEventPayload["data"],
  sequence: number,
): GraphRunEventPayload {
  return {
    planId: "plan-1",
    runId: "run-1",
    workspaceId: "workspace-1",
    sequence,
    timestampMs: sequence,
    event,
    data,
  } as GraphRunEventPayload;
}

function activity(status: string): AgentActivity {
  return {
    id: "tool-1",
    runId: "run-1",
    nodeId: "node-1",
    sequence: 10,
    kind: "tool_call",
    status,
    title: "read",
    content: status,
    payloadJson: "{}",
    startedAt: 1,
    finishedAt: status === "finished" ? 2 : null,
  };
}

describe("graph run reducer", () => {
  it("合并文本 delta，通知保持 100ms 合批语义", () => {
    const state = snapshot();
    const first = reduceGraphRunEvent(
      state,
      payload("nodeOutputDelta", { nodeId: "node-1", delta: "hello " }, 1),
    );
    reduceGraphRunEvent(
      state,
      payload("nodeOutputDelta", { nodeId: "node-1", delta: "PI" }, 2),
    );
    expect(state.liveOutputs["node-1"]).toBe("hello PI");
    expect(first.notification).toBe("throttled");
  });

  it("按活动 id 替换工具增量，工具计数只增加一次", () => {
    const state = snapshot();
    reduceGraphRunEvent(
      state,
      payload("nodeActivity", { nodeId: "node-1", activity: activity("started") }, 1),
    );
    reduceGraphRunEvent(
      state,
      payload("nodeActivity", { nodeId: "node-1", activity: activity("finished") }, 2),
    );
    expect(state.liveActivities["node-1"]).toHaveLength(1);
    expect(state.liveActivities["node-1"][0].status).toBe("finished");
    expect(state.plan?.nodeRuns[0].toolCallCount).toBe(1);
  });

  it("新 attempt 启动时清空上一轮实时缓冲", () => {
    const state = snapshot();
    state.liveOutputs["node-1"] = "old";
    state.liveActivities["node-1"] = [activity("finished")];
    const effect = reduceGraphRunEvent(
      state,
      payload("runStarted", { title: "Plan", attemptNo: 2, nodeCount: 1 }, 3),
    );
    expect(state.liveOutputs).toEqual({});
    expect(state.liveActivities).toEqual({});
    expect(effect.hydrate).toBe(true);
  });

  it("runPaused 置暂停标记且 plan 保持 running，runResumed 复位", () => {
    const state = snapshot();
    reduceGraphRunEvent(state, payload("runPaused", { nodeId: "node-1" }, 1));
    expect(state.paused).toBe(true);
    expect(state.pausedNodeId).toBe("node-1");
    expect(state.plan?.status).toBe("running");

    reduceGraphRunEvent(state, payload("runResumed", {}, 2));
    expect(state.paused).toBe(false);
    expect(state.pausedNodeId).toBeNull();
  });

  it("runFinished 复位暂停标记", () => {
    const state = snapshot();
    reduceGraphRunEvent(state, payload("runPaused", { nodeId: "node-1" }, 1));
    reduceGraphRunEvent(
      state,
      payload("runFinished", { state: {}, failedNodes: [], skippedNodes: [] }, 2),
    );
    expect(state.paused).toBe(false);
    expect(state.pausedNodeId).toBeNull();
  });

  it("nodeCancelled 落 cancelled 状态，与 nodeSkipped 区分", () => {
    const state = snapshot();
    reduceGraphRunEvent(state, payload("nodeCancelled", { nodeId: "node-1" }, 1));
    expect(state.plan?.nodeRuns[0].status).toBe("cancelled");
    expect(state.plan?.nodeRuns[0].finishedAt).toBe(1);
  });
});

describe("serial task queue", () => {
  it("serializes writes and continues after a failed write", async () => {
    const enqueue = createSerialTaskQueue();
    const order: string[] = [];
    let releaseFirst!: () => void;
    const firstGate = new Promise<void>((resolve) => { releaseFirst = resolve; });

    const first = enqueue(async () => {
      order.push("first:start");
      await firstGate;
      order.push("first:end");
    });
    const second = enqueue(async () => {
      order.push("second");
      throw new Error("save failed");
    });
    const third = enqueue(async () => { order.push("third"); });

    await Promise.resolve();
    expect(order).toEqual(["first:start"]);
    releaseFirst();
    await first;
    await expect(second).rejects.toThrow("save failed");
    await third;
    expect(order).toEqual(["first:start", "first:end", "second", "third"]);
  });
});

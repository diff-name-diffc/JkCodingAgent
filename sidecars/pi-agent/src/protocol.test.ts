import assert from "node:assert/strict";
import test from "node:test";

import { parseHostRequest } from "./protocol.ts";

function completeStart(): Record<string, unknown> {
  return {
    type: "start",
    requestId: "request-1",
    runId: "run-1",
    nodeId: "node-1",
    sequence: 1,
    workspace: "/tmp/workspace",
    agentDir: "/tmp/agent",
    projectResourceDir: "/tmp/project-agent",
    prompt: "implement",
    model: { ref: "model-1", url: "http://localhost/v1", apiKey: "secret", model: "test", category: "text" },
    baseToolGroup: "coding",
    specialTools: [],
    hostTools: [],
  };
}

test("parses a complete start frame", () => {
  const request = parseHostRequest(JSON.stringify(completeStart()));
  assert.equal(request.type, "start");
  assert.equal(request.runId, "run-1");
});

test("rejects malformed and unknown frames loudly", () => {
  assert.throws(() => parseHostRequest("not-json"));
  assert.throws(() => parseHostRequest('{"type":"start","requestId":"x"}'), /runId/);
  assert.throws(() => parseHostRequest('{"type":"legacy","requestId":"x","runId":"x","nodeId":"x","sequence":1}'), /未知协议/);
});

test("rejects malformed nested model fields", () => {
  for (const model of [
    { ref: "model-1", url: "http://localhost/v1", apiKey: "secret", category: "text" },
    { ref: "model-1", url: "http://localhost/v1", apiKey: 1, model: "test", category: "text" },
    { ref: "model-1", url: "http://localhost/v1", apiKey: "secret", model: "test", category: "audio" },
  ]) {
    assert.throws(() => parseHostRequest(JSON.stringify({ ...completeStart(), model })), /model|apiKey/);
  }
});

test("rejects malformed tool specifications", () => {
  assert.throws(() => parseHostRequest(JSON.stringify({
    ...completeStart(),
    specialTools: [{ source: "unknown", name: "tool" }],
  })), /specialTools/);
  assert.throws(() => parseHostRequest(JSON.stringify({
    ...completeStart(),
    hostTools: [{ name: "tool", runtimeName: "aha__tool", description: "tool", parameters: [] }],
  })), /hostTools/);
});

test("requires exactly one host tool result payload", () => {
  const frame = {
    type: "host_tool_result",
    requestId: "request-1",
    runId: "run-1",
    nodeId: "node-1",
    sequence: 2,
    callId: "call-1",
  };
  assert.throws(() => parseHostRequest(JSON.stringify(frame)), /result 或 error/);
  assert.throws(() => parseHostRequest(JSON.stringify({ ...frame, result: "ok", error: "failed" })), /result 或 error/);
  assert.throws(() => parseHostRequest(JSON.stringify({ ...frame, error: "" })), /非空字符串/);
  assert.equal(parseHostRequest(JSON.stringify({ ...frame, result: "" })).type, "host_tool_result");
});

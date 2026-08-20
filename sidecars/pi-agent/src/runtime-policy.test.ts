import assert from "node:assert/strict";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  createAgentSession,
  DefaultResourceLoader,
  ModelRuntime,
  SessionManager,
  SettingsManager,
  type ToolDefinition,
} from "@earendil-works/pi-coding-agent";
import type { HostToolSpec, StartRequest } from "./protocol.ts";
import { resolveHostRuntimeNames } from "./runtime-policy.ts";

const parameters = { type: "object", properties: {} };

function tool(runtimeName: string, name: string): HostToolSpec {
  return { runtimeName, name, description: name, parameters };
}

function request(
  baseToolGroup: StartRequest["baseToolGroup"],
  hostTools: HostToolSpec[]
): Pick<StartRequest, "baseToolGroup" | "hostTools" | "specialTools"> {
  return { baseToolGroup, hostTools, specialTools: [] };
}

const readOnly = [
  tool("read", "read_file"),
  tool("grep", "grep"),
  tool("find", "glob"),
  tool("ls", "list_dir"),
];

test("activates only host-backed aliases for read-only and coding groups", () => {
  assert.deepEqual(
    resolveHostRuntimeNames(request("read_only", readOnly)),
    ["read", "grep", "find", "ls"]
  );

  const coding = [
    ...readOnly,
    tool("bash", "exec"),
    tool("edit", "edit_file"),
    tool("write", "write_file"),
    tool("aha__browser_navigate", "browser_navigate"),
  ];
  assert.deepEqual(
    resolveHostRuntimeNames(request("coding", coding)),
    ["read", "grep", "find", "ls", "bash", "edit", "write", "aha__browser_navigate"]
  );
});

test("rejects missing, swapped, or over-granted base aliases", () => {
  assert.throws(
    () => resolveHostRuntimeNames(request("read_only", readOnly.slice(1))),
    /read -> read_file/
  );
  assert.throws(
    () => resolveHostRuntimeNames(request("read_only", [tool("read", "exec"), ...readOnly.slice(1)])),
    /read -> read_file/
  );
  assert.throws(
    () => resolveHostRuntimeNames(request("read_only", [...readOnly, tool("bash", "exec")])),
    /不允许宿主别名 'bash'/
  );
});

test("rejects extension selections and unnamespaced extra tools", () => {
  assert.throws(
    () => resolveHostRuntimeNames({
      ...request("read_only", readOnly),
      specialTools: [{ source: "pi_extension", name: "unsafe" } as never],
    }),
    /可执行扩展已禁用/
  );
  assert.throws(
    () => resolveHostRuntimeNames(request("read_only", [...readOnly, tool("browser", "browser_navigate")])),
    /aha__ 前缀/
  );
});

test("pinned PI SDK resolves a builtin-shaped name to the host custom tool", async () => {
  const cwd = tmpdir();
  const agentDir = join(tmpdir(), "aha-pi-runtime-policy-empty-agent-dir");
  const loader = new DefaultResourceLoader({
    cwd,
    agentDir,
    noExtensions: true,
    noSkills: true,
    noPromptTemplates: true,
    noThemes: true,
    noContextFiles: true,
  });
  await loader.reload();
  const runtime = await ModelRuntime.create({ allowModelNetwork: false });
  const customRead: ToolDefinition = {
    name: "read",
    label: "host read",
    description: "host-backed read",
    parameters,
    execute: async () => ({ content: [{ type: "text", text: "host" }], details: {} }),
  };
  const { session } = await createAgentSession({
    cwd,
    agentDir,
    modelRuntime: runtime,
    resourceLoader: loader,
    customTools: [customRead],
    tools: ["read"],
    sessionManager: SessionManager.inMemory(cwd),
    settingsManager: SettingsManager.inMemory(),
  });
  try {
    const registered = session.getAllTools().find((candidate) => candidate.name === "read");
    assert.equal(registered?.sourceInfo?.source, "sdk");
    assert.deepEqual(session.getActiveToolNames(), ["read"]);
  } finally {
    session.dispose();
  }
});

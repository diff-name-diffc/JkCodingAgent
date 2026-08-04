import { createInterface } from "node:readline";
import { existsSync } from "node:fs";
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import type { Model } from "@earendil-works/pi-ai";
import {
  createAgentSession,
  DefaultResourceLoader,
  loadProjectContextFiles,
  ModelRuntime,
  SessionManager,
  SettingsManager,
  type AgentSessionEvent,
  type ResourceLoader,
  type ToolDefinition
} from "@earendil-works/pi-coding-agent";
import {
  PROTOCOL_VERSION,
  parseHostRequest,
  type DiscoverRequest,
  type HostRequest,
  type HostToolSpec,
  type SidecarMessage,
  type StartRequest
} from "./protocol.js";

// 协议独占原始 stdout；SDK、扩展及 console 的输出统一重定向到 stderr。
const writeProtocol = process.stdout.write.bind(process.stdout) as (chunk: string) => boolean;
Object.defineProperty(process.stdout, "write", {
  value: (chunk: unknown) => process.stderr.write(
    typeof chunk === "string" ? chunk : Buffer.isBuffer(chunk) ? chunk : String(chunk)
  )
});

let sequence = 0;
let currentSession: { abort(): Promise<void>; dispose(): void } | null = null;
const pendingHostTools = new Map<string, { resolve(value: string): void; reject(error: Error): void }>();
const BUILTIN_TOOL_NAMES = new Set(["read", "grep", "find", "ls", "bash", "edit", "write"]);

function send(request: { requestId: string; runId: string; nodeId: string }, type: SidecarMessage["type"], data?: unknown) {
  const message: SidecarMessage = {
    type,
    requestId: request.requestId,
    runId: request.runId,
    nodeId: request.nodeId,
    sequence: ++sequence,
    ...(data === undefined ? {} : { data })
  };
  writeProtocol(`${JSON.stringify(message)}\n`);
}

function serialize(value: unknown): unknown {
  if (value === undefined) return null;
  try {
    return JSON.parse(JSON.stringify(value));
  } catch {
    return String(value);
  }
}

async function createLoader(workspace: string, agentDir: string, projectResourceDir: string): Promise<ResourceLoader> {
  const discoveryCwd = join(agentDir, "discovery-root");
  await mkdir(discoveryCwd, { recursive: true });
  const settingsManager = SettingsManager.inMemory({ compaction: { enabled: true }, retry: { enabled: true, maxRetries: 2 } });
  const existingProjectPaths = (kind: "extensions" | "skills" | "prompts") => {
    const path = join(projectResourceDir, kind);
    return existsSync(path) ? [path] : [];
  };
  const loader = new DefaultResourceLoader({
    cwd: discoveryCwd,
    agentDir,
    settingsManager,
    additionalExtensionPaths: existingProjectPaths("extensions"),
    additionalSkillPaths: existingProjectPaths("skills"),
    additionalPromptTemplatePaths: existingProjectPaths("prompts"),
    agentsFilesOverride: () => ({
      agentsFiles: loadProjectContextFiles({ cwd: workspace, agentDir }).filter(
        (file) => !file.path.includes(`${join(workspace, ".pi")}/`)
      )
    })
  });
  await loader.reload();
  return loader;
}

async function createRuntime(modelConfig?: StartRequest["model"]) {
  const runtime = await ModelRuntime.create({ allowModelNetwork: false });
  const provider = "aha-node";
  const modelId = modelConfig?.model || "catalog-placeholder";
  runtime.registerProvider(provider, {
    name: "Aha Node Runtime",
    baseUrl: modelConfig?.url || "http://127.0.0.1/unused/v1",
    api: "openai-completions",
    models: [{
      id: modelId,
      name: modelConfig?.alias || modelId,
      reasoning: false,
      input: modelConfig?.category === "vision" ? ["text", "image"] : ["text"],
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      contextWindow: 128000,
      maxTokens: 16384
    }]
  });
  if (modelConfig?.apiKey) runtime.setRuntimeApiKey(provider, modelConfig.apiKey);
  const model = runtime.getModel(provider, modelId) as Model<any> | undefined;
  if (!model) throw new Error(`PI 模型注册失败：${modelId}`);
  return { runtime, model };
}

function hostToolDefinition(request: StartRequest, spec: HostToolSpec): ToolDefinition {
  return {
    name: spec.runtimeName,
    label: spec.name,
    description: spec.description,
    parameters: spec.parameters as never,
    execute: async (toolCallId, params, signal, onUpdate) => {
      if (signal?.aborted) throw new Error("工具调用已取消");
      send(request, "host_tool_call", {
        callId: toolCallId,
        name: spec.name,
        runtimeName: spec.runtimeName,
        args: serialize(params)
      });
      const result = await new Promise<string>((resolve, reject) => {
        pendingHostTools.set(toolCallId, { resolve, reject });
        signal?.addEventListener("abort", () => reject(new Error("工具调用已取消")), { once: true });
      });
      onUpdate?.({ content: [{ type: "text", text: result }], details: {} });
      return { content: [{ type: "text", text: result }], details: {} };
    }
  };
}

async function discover(request: DiscoverRequest) {
  const loader = await createLoader(request.workspace, request.agentDir, request.projectResourceDir);
  const { runtime, model } = await createRuntime();
  const { session } = await createAgentSession({
    cwd: request.workspace,
    agentDir: request.agentDir,
    modelRuntime: runtime,
    model,
    thinkingLevel: "off",
    resourceLoader: loader,
    sessionManager: SessionManager.inMemory(request.workspace),
    settingsManager: SettingsManager.inMemory(),
    noTools: "all"
  });
  try {
    const extensionErrors = loader.getExtensions().errors.map(
      ({ path, error }) => `扩展加载失败（${path}）：${error}`
    );
    const resourceDiagnostics = [
      ...loader.getSkills().diagnostics,
      ...loader.getPrompts().diagnostics
    ].map((diagnostic) => diagnostic.path ? `${diagnostic.message}（${diagnostic.path}）` : diagnostic.message);
    const collisions: string[] = [];
    const tools = session.getAllTools()
      .filter((tool) => tool.sourceInfo?.source !== "builtin")
      .filter((tool) => {
        if (!BUILTIN_TOOL_NAMES.has(tool.name)) return true;
        collisions.push(`扩展工具 '${tool.name}' 与 PI 基础工具重名，已排除`);
        return false;
      })
      .map((tool) => ({
        name: tool.name,
        description: tool.description,
        parameters: serialize(tool.parameters),
        source: serialize(tool.sourceInfo)
      }));
    send(request, "catalog", {
      protocolVersion: PROTOCOL_VERSION,
      tools,
      diagnostics: [...extensionErrors, ...resourceDiagnostics, ...collisions]
    });
  } finally {
    session.dispose();
  }
}

function normalizeEvent(
  event: AgentSessionEvent,
  extensionNames: ReadonlySet<string>,
  ahaRuntimeNames: ReadonlySet<string>
): { kind: string; [key: string]: unknown } | null {
  switch (event.type) {
    case "agent_start": return { kind: "lifecycle", phase: "starting" };
    case "agent_settled": return { kind: "lifecycle", phase: "finalizing" };
    case "turn_start": return { kind: "turn", status: "started" };
    case "turn_end": return { kind: "turn", status: "finished", message: serialize(event.message) };
    case "message_update": {
      const update = event.assistantMessageEvent;
      if (update.type === "text_delta") return { kind: "assistant_text", delta: update.delta };
      if (update.type === "thinking_delta") return { kind: "thinking", delta: update.delta };
      if (update.type === "toolcall_start") return { kind: "tool_call_stream", event: update.type };
      if (update.type === "toolcall_delta") return { kind: "tool_call_stream", event: update.type, delta: update.delta };
      if (update.type === "toolcall_end") return { kind: "tool_call_stream", event: update.type, toolCall: serialize(update.toolCall) };
      return null;
    }
    case "tool_execution_start": return { kind: "tool_call", status: "started", source: toolSource(event.toolName, extensionNames, ahaRuntimeNames), callId: event.toolCallId, name: event.toolName, args: serialize(event.args) };
    case "tool_execution_update": return { kind: "tool_call", status: "updated", source: toolSource(event.toolName, extensionNames, ahaRuntimeNames), callId: event.toolCallId, name: event.toolName, result: serialize(event.partialResult) };
    case "tool_execution_end": return { kind: "tool_call", status: event.isError ? "failed" : "finished", source: toolSource(event.toolName, extensionNames, ahaRuntimeNames), callId: event.toolCallId, name: event.toolName, result: serialize(event.result), isError: event.isError };
    case "compaction_start": return { kind: "compaction", status: "started", reason: event.reason };
    case "compaction_end": return { kind: "compaction", status: event.aborted ? "failed" : "finished", reason: event.reason, result: serialize(event.result), error: event.errorMessage };
    case "auto_retry_start": return { kind: "retry", status: "started", attempt: event.attempt, maxAttempts: event.maxAttempts, delayMs: event.delayMs, error: event.errorMessage };
    case "auto_retry_end": return { kind: "retry", status: event.success ? "finished" : "failed", attempt: event.attempt, error: event.finalError };
    default: return null;
  }
}

function toolSource(name: string, extensions: ReadonlySet<string>, aha: ReadonlySet<string>) {
  if (aha.has(name)) return "aha";
  if (extensions.has(name)) return "pi_extension";
  return "pi_builtin";
}

async function start(request: StartRequest) {
  const loader = await createLoader(request.workspace, request.agentDir, request.projectResourceDir);
  const { runtime, model } = await createRuntime(request.model);
  const hostTools = request.hostTools.map((tool) => hostToolDefinition(request, tool));
  const builtin = request.baseToolGroup === "coding"
    ? ["read", "grep", "find", "ls", "bash", "edit", "write"]
    : ["read", "grep", "find", "ls"];
  const extensionNames = request.specialTools
    .filter((tool) => tool.source === "pi_extension")
    .map((tool) => tool.name);
  const ahaRuntimeNames = request.hostTools.map((tool) => tool.runtimeName);
  const extensionNameSet = new Set(extensionNames);
  const ahaRuntimeNameSet = new Set(ahaRuntimeNames);
  const { session } = await createAgentSession({
    cwd: request.workspace,
    agentDir: request.agentDir,
    modelRuntime: runtime,
    model,
    thinkingLevel: "off",
    resourceLoader: loader,
    sessionManager: SessionManager.inMemory(request.workspace),
    settingsManager: SettingsManager.inMemory({ compaction: { enabled: true }, retry: { enabled: true, maxRetries: 2 } }),
    tools: [...builtin, ...extensionNames, ...ahaRuntimeNames],
    customTools: hostTools
  });
  currentSession = session;
  let output = "";
  const unsubscribe = session.subscribe((event) => {
    const normalized = normalizeEvent(event, extensionNameSet, ahaRuntimeNameSet);
    if (!normalized) return;
    if (normalized.kind === "assistant_text" && typeof normalized.delta === "string") output += normalized.delta;
    send(request, "agent_event", normalized);
  });
  try {
    await session.prompt(request.prompt);
    await session.waitForIdle();
    send(request, "completed", { output, usage: session.getSessionStats().tokens });
  } finally {
    unsubscribe();
    session.dispose();
    currentSession = null;
  }
}

async function handle(request: HostRequest) {
  if (request.type === "cancel") {
    await currentSession?.abort();
    return;
  }
  if (request.type === "host_tool_result") {
    const pending = pendingHostTools.get(request.callId);
    if (!pending) return;
    pendingHostTools.delete(request.callId);
    request.error ? pending.reject(new Error(request.error)) : pending.resolve(request.result ?? "");
    return;
  }
  try {
    if (request.type === "discover") await discover(request);
    else await start(request);
  } catch (error) {
    send(request, "failed", { error: error instanceof Error ? error.message : String(error) });
  }
}

send({ requestId: "sidecar", runId: "sidecar", nodeId: "sidecar" }, "ready", { protocolVersion: PROTOCOL_VERSION });
const reader = createInterface({ input: process.stdin, crlfDelay: Infinity });
let lastHostSequence = 0;
reader.on("line", (line) => {
  const trimmed = line.trim();
  if (!trimmed) return;
  let request: HostRequest;
  try {
    request = parseHostRequest(trimmed);
    if (request.sequence <= lastHostSequence) {
      throw new Error(`协议 sequence 非单调递增：${request.sequence} <= ${lastHostSequence}`);
    }
    lastHostSequence = request.sequence;
  } catch (error) {
    process.stderr.write(`无效 JSONL：${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
    reader.close();
    return;
  }
  void handle(request);
});

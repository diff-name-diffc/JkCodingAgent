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
  type HostRequest,
  type HostToolSpec,
  type SidecarMessage,
  type StartRequest
} from "./protocol.js";
import { resolveHostRuntimeNames } from "./runtime-policy.js";

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

// PI 内置系统提示词为英文（"You are an expert coding assistant…"），会把节点的
// 实时叙述与最终产出带成英文。节点输入（需求/角色/子任务）都是中文，这里通过
// SDK 的 appendSystemPrompt 在系统提示词层追加语言约定——比重写 systemPrompt
// 更稳妥（保留 PI 的工具使用指引），也比塞进每个节点的用户输入更强效。
const OUTPUT_LANGUAGE_DIRECTIVE = [
  "# 输出语言",
  "你的所有叙述性输出（实时响应、思考说明、最终的「## 产出摘要」等全部分区）必须使用简体中文。",
  "代码、命令、文件路径、标识符、API 名称等技术内容保持原文，不要翻译。",
  "无论上游节点输出使用何种语言，你的输出语言要求不变。"
].join("\n");

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
  const existingProjectPaths = (kind: "skills" | "prompts") => {
    const path = join(projectResourceDir, kind);
    return existsSync(path) ? [path] : [];
  };
  const loader = new DefaultResourceLoader({
    cwd: discoveryCwd,
    agentDir,
    settingsManager,
    // 用 override 追加而非 appendSystemPrompt 直设：后者会让 SDK 跳过
    // APPEND_SYSTEM.md 的发现逻辑，覆盖用户/项目级追加提示词。
    // 签名依据 @earendil-works/pi-coding-agent@0.83.0
    // （resource-loader.d.ts：`appendSystemPromptOverride?: (base: string[]) => string[]`，
    // 且 override 作用于 APPEND_SYSTEM.md 发现结果之上）；Array.isArray 防御
    // SDK 升级漂移——base 若退化为 string，展开会逐字符拆分成错误提示词。
    appendSystemPromptOverride: (base) => [
      ...(Array.isArray(base) ? base : []),
      OUTPUT_LANGUAGE_DIRECTIVE
    ],
    // 扩展是任意 Node.js 代码，加载阶段就可能产生副作用，无法被宿主
    // CapabilityBroker 审查。执行图因此完全禁用扩展；skills/prompts 仍是
    // 声明式文本资源，可以安全加载。
    noExtensions: true,
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

async function createRuntime(modelConfig: StartRequest["model"]) {
  const runtime = await ModelRuntime.create({ allowModelNetwork: false });
  const provider = "aha-node";
  const modelId = modelConfig.model;
  runtime.registerProvider(provider, {
    name: "Aha Node Runtime",
    baseUrl: modelConfig.url,
    api: "openai-completions",
    models: [{
      id: modelId,
      name: modelConfig.alias || modelId,
      reasoning: false,
      input: modelConfig.category === "vision" ? ["text", "image"] : ["text"],
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      contextWindow: 128000,
      maxTokens: 16384
    }]
  });
  if (modelConfig.apiKey) runtime.setRuntimeApiKey(provider, modelConfig.apiKey);
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

function normalizeEvent(
  event: AgentSessionEvent,
  hostRuntimeNames: ReadonlySet<string>
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
    case "tool_execution_start": return { kind: "tool_call", status: "started", source: toolSource(event.toolName, hostRuntimeNames), callId: event.toolCallId, name: event.toolName, args: serialize(event.args) };
    case "tool_execution_update": return { kind: "tool_call", status: "updated", source: toolSource(event.toolName, hostRuntimeNames), callId: event.toolCallId, name: event.toolName, result: serialize(event.partialResult) };
    case "tool_execution_end": return { kind: "tool_call", status: event.isError ? "failed" : "finished", source: toolSource(event.toolName, hostRuntimeNames), callId: event.toolCallId, name: event.toolName, result: serialize(event.result), isError: event.isError };
    case "compaction_start": return { kind: "compaction", status: "started", reason: event.reason };
    case "compaction_end": return { kind: "compaction", status: event.aborted ? "failed" : "finished", reason: event.reason, result: serialize(event.result), error: event.errorMessage };
    case "auto_retry_start": return { kind: "retry", status: "started", attempt: event.attempt, maxAttempts: event.maxAttempts, delayMs: event.delayMs, error: event.errorMessage };
    case "auto_retry_end": return { kind: "retry", status: event.success ? "finished" : "failed", attempt: event.attempt, error: event.finalError };
    default: return null;
  }
}

function toolSource(name: string, hostRuntimeNames: ReadonlySet<string>) {
  return hostRuntimeNames.has(name) ? "host" : "unexpected";
}

/** 上下文占用采样间隔：估算需遍历会话消息，不能随高频 delta 事件全量执行。 */
const CONTEXT_USAGE_SAMPLE_INTERVAL_MS = 1_000;

type ContextUsageSession = {
  getContextUsage(): { tokens: number | null; contextWindow: number; percent: number | null } | undefined;
};

/**
 * 运行期上下文占用采样：随 session 事件节流上报（数值变化才发）。
 * compaction 后、下一次 LLM 响应前 SDK 会返回 tokens=null，原样透传给宿主
 * 展示「重新估算中」；force 用于收尾时兜底上报一次最终读数。
 */
function createContextUsageSampler(
  request: StartRequest,
  session: ContextUsageSession
): { sample(force?: boolean): void } {
  let lastSampleAt = 0;
  let lastSignature = "";
  return {
    sample(force = false) {
      const now = Date.now();
      if (!force && now - lastSampleAt < CONTEXT_USAGE_SAMPLE_INTERVAL_MS) return;
      const usage = session.getContextUsage();
      if (!usage) return;
      // 确认读数有效后再刷新节流窗口：否则边界处的瞬态 undefined 会白白吞掉一个窗口。
      lastSampleAt = now;
      const payload = { tokens: usage.tokens, contextWindow: usage.contextWindow, percent: usage.percent };
      const signature = JSON.stringify(payload);
      if (signature === lastSignature) return;
      lastSignature = signature;
      send(request, "agent_event", { kind: "context_usage", ...payload });
    }
  };
}

async function start(request: StartRequest) {
  const loader = await createLoader(request.workspace, request.agentDir, request.projectResourceDir);
  const { runtime, model } = await createRuntime(request.model);
  const hostTools = request.hostTools.map((tool) => hostToolDefinition(request, tool));
  const hostRuntimeNames = resolveHostRuntimeNames(request);
  const hostRuntimeNameSet = new Set(hostRuntimeNames);
  const { session } = await createAgentSession({
    cwd: request.workspace,
    agentDir: request.agentDir,
    modelRuntime: runtime,
    model,
    thinkingLevel: "off",
    resourceLoader: loader,
    sessionManager: SessionManager.inMemory(request.workspace),
    settingsManager: SettingsManager.inMemory({ compaction: { enabled: true }, retry: { enabled: true, maxRetries: 2 } }),
    // 激活列表只来自宿主下发的 customTools。read/bash 等名称虽与 PI builtin
    // 相同，但 SDK 的 customTools 会覆盖同名 builtin 定义与实现。
    tools: hostRuntimeNames,
    customTools: hostTools
  });
  currentSession = session;
  let output = "";
  const usageSampler = createContextUsageSampler(request, session);
  const unsubscribe = session.subscribe((event) => {
    const normalized = normalizeEvent(event, hostRuntimeNameSet);
    if (!normalized) return;
    if (normalized.kind === "assistant_text" && typeof normalized.delta === "string") output += normalized.delta;
    send(request, "agent_event", normalized);
    usageSampler.sample();
  });
  try {
    await session.prompt(request.prompt);
    await session.waitForIdle();
    send(request, "completed", { output, usage: session.getSessionStats().tokens });
  } catch (error) {
    send(request, "failed", {
      error: error instanceof Error ? error.message : String(error),
      usage: session.getSessionStats().tokens,
    });
  } finally {
    // 兜底采样放在 finally 而非成功路径：prompt 抛错走 failed 分支时同样上报
    // 最终上下文占用，避免前端读数停留在过期值（宿主侧 failed 分支不补发）。
    usageSampler.sample(true);
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
    await start(request);
  } catch (error) {
    send(request, "failed", { error: error instanceof Error ? error.message : String(error) });
  }
}

send({ requestId: "sidecar", runId: "sidecar", nodeId: "sidecar" }, "ready", { protocolVersion: PROTOCOL_VERSION });
const reader = createInterface({ input: process.stdin, crlfDelay: Infinity });
// stdin EOF 即宿主进程已消失（崩溃/被 SIGKILL，宿主侧 kill_on_drop 不会执行）。
// 没有此处理时，等待中的 LLM 请求或永不到达的 host_tool_result 会让事件循环
// 永远挂起，sidecar 成为孤儿进程：先中止在途会话，宽限期后强制退出。
reader.on("close", () => {
  // stdin EOF 意味着宿主已消失，属异常收尾：以非零退出码区别于「正常结束」。
  // 先行置位：unref 的宽限期定时器不保持事件循环，若 abort() 挂起且背后没有
  // 活跃 I/O 句柄，事件循环会在宽限期前自然耗尽——此时只有 exitCode 能保证
  // 仍以非零码退出，与孤儿语义一致。
  process.exitCode = 1;
  // abort() 的 rejection 必须显式吞掉：finally 不拦截原 promise 的 rejection，
  // 直接 void 掉会触发 unhandledRejection，进程以未捕获异常方式退出而非干净退出。
  const graceful = (currentSession?.abort() ?? Promise.resolve()).catch(() => {});
  // 退出前向 stderr 留痕（stdout 未 drain 的协议消息已无消费者）。
  const exitOrphaned = () => {
    process.stderr.write("宿主 stdin 已关闭（宿主进程消失），sidecar 退出\n");
    process.exit(1);
  };
  void graceful.then(exitOrphaned);
  setTimeout(exitOrphaned, 3_000).unref();
});
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

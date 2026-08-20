export const PROTOCOL_VERSION = 3;

export type ModelConfig = {
  ref: string;
  url: string;
  apiKey: string;
  model: string;
  category: "text" | "vision";
  alias?: string;
};

export type HostToolSpec = {
  name: string;
  runtimeName: string;
  description: string;
  parameters: Record<string, unknown>;
};

export type ToolRef = { source: "aha"; name: string };

type Envelope = { requestId: string; runId: string; nodeId: string; sequence: number };

export type StartRequest = Envelope & {
  type: "start";
  runId: string;
  nodeId: string;
  workspace: string;
  agentDir: string;
  projectResourceDir: string;
  prompt: string;
  model: ModelConfig;
  baseToolGroup: "read_only" | "coding";
  specialTools: ToolRef[];
  hostTools: HostToolSpec[];
};

export type HostRequest = StartRequest |
  (Envelope & { type: "cancel" }) |
  (Envelope & { type: "host_tool_result"; callId: string; result?: string; error?: string });

export type SidecarMessage = Envelope & {
  type: "ready" | "agent_event" | "host_tool_call" | "completed" | "failed";
  data?: unknown;
};

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function stringField(value: Record<string, unknown>, key: string): string {
  const field = value[key];
  if (typeof field !== "string" || field.length === 0) throw new Error(`协议字段 ${key} 必须是非空字符串`);
  return field;
}

function stringValue(value: Record<string, unknown>, key: string): string {
  const field = value[key];
  if (typeof field !== "string") throw new Error(`协议字段 ${key} 必须是字符串`);
  return field;
}

function validateModel(value: unknown): asserts value is ModelConfig {
  if (!record(value)) throw new Error("start 消息缺少 model");
  for (const key of ["ref", "url", "model"]) stringField(value, key);
  stringValue(value, "apiKey");
  if (value.category !== "text" && value.category !== "vision") {
    throw new Error("model.category 非法");
  }
  if (value.alias !== undefined && typeof value.alias !== "string") {
    throw new Error("model.alias 必须是字符串");
  }
}

function validateSpecialTools(value: unknown): asserts value is ToolRef[] {
  if (!Array.isArray(value)) throw new Error("start 消息缺少 specialTools");
  value.forEach((tool, index) => {
    if (!record(tool)) throw new Error(`specialTools[${index}] 必须是对象`);
    if (tool.source === "pi_extension") {
      throw new Error(`specialTools[${index}] 引用了已禁用的 PI 可执行扩展`);
    }
    if (tool.source !== "aha") {
      throw new Error(`specialTools[${index}].source 非法`);
    }
    stringField(tool, "name");
  });
}

function validateHostTools(value: unknown): asserts value is HostToolSpec[] {
  if (!Array.isArray(value)) throw new Error("start 消息缺少 hostTools");
  const names = new Set<string>();
  const runtimeNames = new Set<string>();
  value.forEach((tool, index) => {
    if (!record(tool)) throw new Error(`hostTools[${index}] 必须是对象`);
    const name = stringField(tool, "name");
    const runtimeName = stringField(tool, "runtimeName");
    stringValue(tool, "description");
    if (!record(tool.parameters)) throw new Error(`hostTools[${index}].parameters 必须是对象`);
    if (names.has(name)) throw new Error(`hostTools[${index}].name 重复：${name}`);
    if (runtimeNames.has(runtimeName)) {
      throw new Error(`hostTools[${index}].runtimeName 重复：${runtimeName}`);
    }
    names.add(name);
    runtimeNames.add(runtimeName);
  });
}

export function parseHostRequest(line: string): HostRequest {
  const value: unknown = JSON.parse(line);
  if (!record(value)) throw new Error("协议消息必须是 JSON 对象");
  const type = stringField(value, "type");
  for (const key of ["requestId", "runId", "nodeId"]) stringField(value, key);
  if (!Number.isSafeInteger(value.sequence) || (value.sequence as number) <= 0) {
    throw new Error("协议字段 sequence 必须是正整数");
  }
  if (type === "start") {
    for (const key of ["runId", "nodeId", "workspace", "agentDir", "projectResourceDir", "prompt", "baseToolGroup"]) stringField(value, key);
    validateModel(value.model);
    validateSpecialTools(value.specialTools);
    validateHostTools(value.hostTools);
    if (value.baseToolGroup !== "read_only" && value.baseToolGroup !== "coding") {
      throw new Error("baseToolGroup 非法");
    }
  } else if (type === "host_tool_result") {
    stringField(value, "callId");
    const hasResult = typeof value.result === "string";
    const hasError = typeof value.error === "string";
    if (hasResult === hasError) throw new Error("host_tool_result 必须且只能包含 result 或 error 字符串");
    if (hasError && (value.error as string).length === 0) throw new Error("host_tool_result.error 必须是非空字符串");
  } else if (type !== "cancel") {
    throw new Error(`未知协议消息类型：${type}`);
  }
  return value as HostRequest;
}

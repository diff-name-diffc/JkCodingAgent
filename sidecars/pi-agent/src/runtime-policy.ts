import type { HostToolSpec, StartRequest } from "./protocol.js";

const READ_ONLY_BASE_TOOLS = [
  ["read", "read_file"],
  ["grep", "grep"],
  ["find", "glob"],
  ["ls", "list_dir"],
] as const;

const CODING_BASE_TOOLS = [
  ["bash", "exec"],
  ["edit", "edit_file"],
  ["write", "write_file"],
] as const;

const RESERVED_PI_TOOL_NAMES = new Set<string>([
  ...READ_ONLY_BASE_TOOLS.map(([runtimeName]) => runtimeName),
  ...CODING_BASE_TOOLS.map(([runtimeName]) => runtimeName),
]);

/**
 * 验证 Rust 下发的宿主工具面，并返回唯一可激活的运行时工具名。
 *
 * PI builtin 与 extension 都不在返回值来源中：同名 builtin 即使被 SDK 构造，
 * 也会被 customTools 的宿主代理定义覆盖，模型的每次调用都只能走 RPC。
 */
export function resolveHostRuntimeNames(
  request: Pick<StartRequest, "baseToolGroup" | "hostTools" | "specialTools">
): string[] {
  if (request.specialTools.some((tool) => (tool as { source: string }).source !== "aha")) {
    throw new Error("PI 可执行扩展已禁用；specialTools 只能引用 Aha 工具");
  }

  const required = request.baseToolGroup === "coding"
    ? [...READ_ONLY_BASE_TOOLS, ...CODING_BASE_TOOLS]
    : [...READ_ONLY_BASE_TOOLS];
  const byRuntimeName = uniqueHostTools(request.hostTools);

  for (const [runtimeName, capabilityName] of required) {
    const tool = byRuntimeName.get(runtimeName);
    if (!tool || tool.name !== capabilityName) {
      throw new Error(
        `基础宿主工具映射缺失或不匹配：${runtimeName} -> ${capabilityName}`
      );
    }
  }

  const requiredRuntimeNames = new Set<string>(required.map(([runtimeName]) => runtimeName));
  for (const tool of request.hostTools) {
    if (RESERVED_PI_TOOL_NAMES.has(tool.runtimeName)) {
      if (!requiredRuntimeNames.has(tool.runtimeName)) {
        throw new Error(`工具组 '${request.baseToolGroup}' 不允许宿主别名 '${tool.runtimeName}'`);
      }
      continue;
    }
    if (!tool.runtimeName.startsWith("aha__")) {
      throw new Error(`附加宿主工具必须使用 aha__ 前缀：${tool.runtimeName}`);
    }
  }

  return request.hostTools.map((tool) => tool.runtimeName);
}

function uniqueHostTools(hostTools: HostToolSpec[]): Map<string, HostToolSpec> {
  const byRuntimeName = new Map<string, HostToolSpec>();
  const capabilityNames = new Set<string>();
  for (const tool of hostTools) {
    if (byRuntimeName.has(tool.runtimeName)) {
      throw new Error(`宿主工具运行名重复：${tool.runtimeName}`);
    }
    if (capabilityNames.has(tool.name)) {
      throw new Error(`宿主能力名重复：${tool.name}`);
    }
    byRuntimeName.set(tool.runtimeName, tool);
    capabilityNames.add(tool.name);
  }
  return byRuntimeName;
}

import type { McpAggregateStatus, McpStatus } from "../types";

/**
 * MCP 工具名契约（后端 `mcp/registry.rs` 的 `resolve_mcp_tool`）：
 * canonical 名恒为 `mcp__<server>__<tool>`，内置工具名不会以该前缀开头，
 * 因此允许列表（内置与 MCP 共用同一数组）可按前缀区分两类工具。
 */
export const MCP_TOOL_NAME_PREFIX = "mcp__";

export function isMcpToolName(name: string): boolean {
  return name.startsWith(MCP_TOOL_NAME_PREFIX);
}

/** 从允许列表中取出 MCP 工具名集合（内置工具名自然被排除）。 */
export function extractMcpToolNames(allowedTools: readonly string[]): Set<string> {
  return new Set(allowedTools.filter(isMcpToolName));
}

/**
 * 按已配置的工具名裁剪 MCP 状态：只保留至少含一个已配置工具的服务器，
 * 服务器内只保留已配置的工具，并重算计数与总体健康度。
 * 供聊天页把全局状态收敛成「当前分类视图」，使指示器与弹层只反映
 * 该分类显式配置的 MCP 工具。
 *
 * 输入为空、名单为空或裁剪后无任何服务器/工具时返回 null。
 */
export function trimMcpStatusToTools(
  status: McpStatus | null,
  names: Set<string>,
): McpStatus | null {
  if (!status || names.size === 0) return null;

  const servers = status.servers
    .map((server) => ({
      ...server,
      tools: server.tools.filter((tool) => names.has(tool.exposedName)),
    }))
    .filter((server) => server.tools.length > 0)
    .map((server) => ({ ...server, toolCount: server.tools.length }));
  if (servers.length === 0) return null;

  const enabledServerCount = servers.filter((server) => server.enabled).length;
  const healthyServerCount = servers.filter((server) => server.state === "healthy").length;
  const aggregate: McpAggregateStatus =
    healthyServerCount === servers.length ? "healthy" : "degraded";

  return {
    ...status,
    aggregate,
    serverCount: servers.length,
    enabledServerCount,
    healthyServerCount,
    servers,
  };
}

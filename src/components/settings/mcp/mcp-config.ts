import type { McpConfig, McpServerConfig } from "../../../types";

/**
 * MCP 全局注册表编辑器的纯逻辑层（UI-22a 从 McpServersPage 抽离）。
 * 条目 ↔ 配置转换、行解析、命名递增、JSON 文本解析/序列化——全部无副作用，
 * 供页面组件与服务器卡片共享，并独立单测。
 */

export const AUTOSAVE_DELAY_MS = 400;

export type TransportKind = "stdio" | "streamable_http" | "unix_socket_http";

export const TRANSPORT_LABELS: Record<TransportKind, string> = {
  stdio: "本地进程（stdio）",
  streamable_http: "HTTP（streamable）",
  unix_socket_http: "Unix socket（HTTP）",
};

export interface McpEntry {
  name: string;
  server: McpServerConfig;
}

export const EMPTY_SERVER: McpServerConfig = {
  enabled: true,
  transport: "stdio",
  command: "",
  args: [],
  env: {},
  headers: {},
};

/** 条目列表 ↔ mcpServers Record 的双向转换（保持插入顺序）。 */
export function toEntries(config: McpConfig): McpEntry[] {
  return Object.entries(config.mcpServers ?? {}).map(([name, server]) => ({
    name,
    server: {
      ...server,
      args: server.args ?? [],
      env: server.env ?? {},
      headers: server.headers ?? {},
    },
  }));
}

export function toConfig(entries: McpEntry[]): McpConfig {
  const mcpServers: Record<string, McpServerConfig> = {};
  for (const { name, server } of entries) {
    const key = name.trim();
    if (!key) continue;
    mcpServers[key] = server;
  }
  return { mcpServers };
}

export function parseLines(raw: string): string[] {
  return raw
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

export function parseKeyValueLines(raw: string, separator: RegExp): Record<string, string> {
  const map: Record<string, string> = {};
  for (const line of parseLines(raw)) {
    const match = separator.exec(line);
    if (match && match[1]?.trim()) {
      map[match[1].trim()] = line.slice(match[0].length).trim();
    }
  }
  return map;
}

export function nextServerName(entries: McpEntry[]): string {
  let index = entries.length + 1;
  let name = `mcp-${index}`;
  const used = new Set(entries.map((entry) => entry.name));
  while (used.has(name)) {
    index += 1;
    name = `mcp-${index}`;
  }
  return name;
}

export type EditorMode = "form" | "json";

export interface ParsedConfigText {
  config: McpConfig | null;
  error: string | null;
}

/**
 * 解析用户编辑的配置 JSON。顶层必须是对象；`mcpServers` 缺失视为空注册表，
 * 兼容后端别名键 `servers`（与项目级 mcp.json 同形状）。字段级形状交给
 * 后端 serde 校验——客户端只做结构防护，避免把非法文本送进保存流程。
 */
export function parseConfigText(text: string): ParsedConfigText {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (err) {
    return {
      config: null,
      error: `JSON 解析失败：${err instanceof Error ? err.message : String(err)}`,
    };
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    return { config: null, error: "配置顶层必须是一个 JSON 对象" };
  }
  const root = parsed as Record<string, unknown>;
  const servers = root.mcpServers ?? root.servers;
  if (
    servers !== undefined &&
    (typeof servers !== "object" || servers === null || Array.isArray(servers))
  ) {
    return { config: null, error: "mcpServers 必须是一个 JSON 对象（服务器名 → 服务器配置）" };
  }
  for (const [name, server] of Object.entries((servers as Record<string, unknown>) ?? {})) {
    if (typeof server !== "object" || server === null || Array.isArray(server)) {
      return { config: null, error: `服务器「${name}」的配置必须是一个 JSON 对象` };
    }
  }
  return {
    config: { mcpServers: (servers as Record<string, McpServerConfig>) ?? {} },
    error: null,
  };
}

export function serializeConfig(config: McpConfig): string {
  return `${JSON.stringify(config, null, 2)}\n`;
}

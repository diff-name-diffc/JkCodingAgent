import { describe, expect, it } from "vitest";
import type { McpServerStatus, McpStatus, McpToolStatus } from "../types";
import { extractMcpToolNames, isMcpToolName, trimMcpStatusToTools } from "./mcp-category-tools";

function tool(exposedName: string): McpToolStatus {
  return {
    name: exposedName.split("__").pop() ?? exposedName,
    exposedName,
    description: `${exposedName} desc`,
    taskSupport: "optional",
  };
}

function server(
  name: string,
  tools: McpToolStatus[],
  state: McpServerStatus["state"] = "healthy",
): McpServerStatus {
  return {
    name,
    transport: "stdio",
    enabled: state !== "disabled",
    state,
    summary: state === "healthy" ? "正常" : "异常",
    toolCount: tools.length,
    tools,
  };
}

function status(servers: McpServerStatus[]): McpStatus {
  const healthyServerCount = servers.filter((server) => server.state === "healthy").length;
  return {
    scope: "global",
    aggregate: healthyServerCount === servers.length ? "healthy" : "degraded",
    checkedAt: 1_700_000_000_000,
    serverCount: servers.length,
    enabledServerCount: servers.filter((server) => server.enabled).length,
    healthyServerCount,
    servers,
  };
}

describe("isMcpToolName / extractMcpToolNames", () => {
  it("splits mcp__ names from builtin names in a shared allowlist", () => {
    expect(isMcpToolName("mcp__any_file_server__list_files")).toBe(true);
    expect(isMcpToolName("local_zsh")).toBe(false);

    const names = extractMcpToolNames([
      "local_zsh",
      "mcp__any_file_server__list_files",
      "browser_read_text",
      "mcp__srv__tool",
    ]);
    expect(names).toEqual(new Set(["mcp__any_file_server__list_files", "mcp__srv__tool"]));
  });

  it("returns an empty set for empty or builtin-only lists", () => {
    expect(extractMcpToolNames([]).size).toBe(0);
    expect(extractMcpToolNames(["local_zsh"]).size).toBe(0);
  });
});

describe("trimMcpStatusToTools", () => {
  const full = status([
    server("files", [tool("mcp__files__list"), tool("mcp__files__read")]),
    server("web", [tool("mcp__web__fetch")], "connection_failed"),
  ]);

  it("keeps only configured tools and drops servers without any", () => {
    const trimmed = trimMcpStatusToTools(full, new Set(["mcp__files__list"]));
    expect(trimmed).not.toBeNull();
    expect(trimmed!.servers).toHaveLength(1);
    expect(trimmed!.servers[0].name).toBe("files");
    expect(trimmed!.servers[0].tools.map((tool) => tool.exposedName)).toEqual([
      "mcp__files__list",
    ]);
    expect(trimmed!.servers[0].toolCount).toBe(1);
  });

  it("recomputes counts and aggregate from the trimmed servers", () => {
    const trimmed = trimMcpStatusToTools(full, new Set(["mcp__web__fetch"]))!;
    expect(trimmed.serverCount).toBe(1);
    expect(trimmed.enabledServerCount).toBe(1);
    expect(trimmed.healthyServerCount).toBe(0);
    expect(trimmed.aggregate).toBe("degraded");

    const healthy = trimMcpStatusToTools(full, new Set(["mcp__files__list", "mcp__files__read"]))!;
    expect(healthy.healthyServerCount).toBe(1);
    expect(healthy.aggregate).toBe("healthy");
  });

  it("returns null when nothing is configured or nothing matches", () => {
    expect(trimMcpStatusToTools(full, new Set())).toBeNull();
    expect(trimMcpStatusToTools(full, new Set(["mcp__gone__tool"]))).toBeNull();
    expect(trimMcpStatusToTools(null, new Set(["mcp__files__list"]))).toBeNull();
  });
});

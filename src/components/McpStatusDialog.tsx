import { useEffect, useState } from "react";
import { ChevronDown, ChevronRight, RefreshCw, X } from "lucide-react";
import type {
  ProjectMcpStatus,
  ProjectMcpServerState,
  ProjectMcpToolTaskSupport,
} from "../types";

function formatTimestamp(timestamp: number): string {
  if (!timestamp) return "未检查";
  return new Date(timestamp).toLocaleString();
}

function serverStateLabel(state: ProjectMcpServerState): string {
  switch (state) {
    case "disabled":
      return "已禁用";
    case "healthy":
      return "正常";
    case "invalid_config":
      return "配置无效";
    case "spawn_failed":
      return "启动失败";
    case "connection_failed":
      return "连接失败";
  }
}

function taskSupportLabel(taskSupport: ProjectMcpToolTaskSupport): string {
  switch (taskSupport) {
    case "required":
      return "task 必需";
    case "optional":
      return "task 可选";
    case "forbidden":
    default:
      return "普通调用";
  }
}

function stateColor(state: ProjectMcpServerState): string {
  switch (state) {
    case "disabled":
      return "var(--text-hint)";
    case "healthy":
      return "var(--success)";
    case "invalid_config":
      return "var(--danger)";
    case "spawn_failed":
      return "var(--warning)";
    case "connection_failed":
      return "var(--danger)";
  }
}

function toolKey(serverName: string, toolName: string): string {
  return `${serverName}::${toolName}`;
}

export function McpStatusDialog({
  projectPath,
  status,
  checking,
  updatingServer,
  onRefresh,
  onToggleServerEnabled,
  onClose,
}: {
  projectPath: string;
  status: ProjectMcpStatus | null;
  checking: boolean;
  updatingServer: string | null;
  onRefresh: () => void;
  onToggleServerEnabled: (serverName: string, enabled: boolean) => void;
  onClose: () => void;
}) {
  const [expandedServers, setExpandedServers] = useState<Record<string, boolean>>({});
  const [expandedTools, setExpandedTools] = useState<Record<string, boolean>>({});

  useEffect(() => {
    if (!status) return;

    const serverNames = new Set(status.servers.map((server) => server.name));
    const validToolKeys = new Set(
      status.servers.flatMap((server) =>
        server.tools.map((tool) => toolKey(server.name, tool.exposedName)),
      ),
    );

    setExpandedServers((current) =>
      Object.fromEntries(
        Object.entries(current).filter(([serverName]) => serverNames.has(serverName)),
      ),
    );
    setExpandedTools((current) =>
      Object.fromEntries(
        Object.entries(current).filter(([key]) => validToolKeys.has(key)),
      ),
    );
  }, [status]);

  return (
    <div className="ai-mcp-overlay" onClick={onClose}>
      <div className="ai-mcp-dialog ai-migrated-mcp-dialog" onClick={(event) => event.stopPropagation()}>
        <div className="ai-mcp-header">
          <div>
            <div className="ai-mcp-title">项目级 MCP 状态</div>
            <div className="ai-mcp-subtitle">
              这里只支持启用或禁用 MCP server；其余配置仍通过 `.jkcodingagent/mcp.json` 管理
            </div>
          </div>
          <div className="ai-mcp-header-actions">
            <button className="ai-mcp-header-button" onClick={onRefresh} disabled={checking}>
              <RefreshCw
                size={14}
                className={checking ? "ai-mcp-spin" : undefined}
              />
              重新检查
            </button>
            <button className="ai-mcp-icon-button" onClick={onClose} title="关闭">
              <X size={16} />
            </button>
          </div>
        </div>

        {status ? (
          <div className="ai-mcp-body chat-scroll">
            <div className="ai-mcp-meta-card">
              <div className="ai-mcp-meta-row">
                <span className="ai-mcp-meta-label">工作区</span>
                <span className="ai-mcp-meta-value">{projectPath}</span>
              </div>
              <div className="ai-mcp-meta-row">
                <span className="ai-mcp-meta-label">配置文件</span>
                <span className="ai-mcp-meta-value">{status.configPath}</span>
              </div>
              <div className="ai-mcp-meta-row">
                <span className="ai-mcp-meta-label">最近检查</span>
                <span className="ai-mcp-meta-value">{formatTimestamp(status.checkedAt)}</span>
              </div>
              <div className="ai-mcp-meta-row">
                <span className="ai-mcp-meta-label">健康度</span>
                <span className="ai-mcp-meta-value">
                  {status.healthyServerCount}/{status.enabledServerCount} 个已启用 server 可用
                  {status.serverCount !== status.enabledServerCount
                    ? `（共 ${status.serverCount} 个配置）`
                    : ""}
                </span>
              </div>
              {status.configError && <div className="ai-mcp-error">{status.configError}</div>}
            </div>

            {status.servers.length === 0 ? (
              <div className="ai-mcp-empty">
                当前没有配置任何 MCP server。请直接编辑 `.jkcodingagent/mcp.json`
                后重新进入项目或点“重新检查”。
              </div>
            ) : (
              <div className="ai-mcp-server-list">
                {status.servers.map((server) => {
                  const expanded = expandedServers[server.name] ?? false;
                  const busy = updatingServer === server.name;
                  const stateColorValue = stateColor(server.state);

                  return (
                    <div key={server.name} className="ai-mcp-server-card">
                      <div className="ai-mcp-server-top">
                        <button
                          type="button"
                          className="ai-mcp-server-toggle"
                          onClick={() =>
                            setExpandedServers((current) => ({
                              ...current,
                              [server.name]: !expanded,
                            }))
                          }
                        >
                          <span className="ai-mcp-chevron">
                            {expanded ? <ChevronDown size={16} /> : <ChevronRight size={16} />}
                          </span>
                          <span
                            className="ai-mcp-server-dot"
                            style={{ background: stateColorValue }}
                          />
                          <span className="ai-mcp-server-info">
                            <span className="ai-mcp-server-title">{server.name}</span>
                            <span className="ai-mcp-server-meta">
                              {server.transport} · {server.toolCount} 个工具 ·{" "}
                              {serverStateLabel(server.state)}
                            </span>
                          </span>
                        </button>

                        <div className="ai-mcp-server-actions">
                          <span
                            className="ai-mcp-state-badge"
                            style={{
                              color: stateColorValue,
                              borderColor: `color-mix(in srgb, ${stateColorValue} 20%, transparent)`,
                              background: `color-mix(in srgb, ${stateColorValue} 8%, transparent)`,
                            }}
                          >
                            {server.summary}
                          </span>

                          <button
                            type="button"
                            role="switch"
                            aria-checked={server.enabled}
                            aria-label={`${server.name} ${server.enabled ? "已启用" : "已禁用"}`}
                            disabled={busy || checking}
                            className={server.enabled ? "ai-mcp-switch is-on" : "ai-mcp-switch"}
                            onClick={(event) => {
                              event.stopPropagation();
                              onToggleServerEnabled(server.name, !server.enabled);
                            }}
                          >
                            <span className="ai-mcp-switch-label">
                              {busy ? "保存中" : server.enabled ? "启用" : "禁用"}
                            </span>
                            <span
                              className="ai-mcp-switch-thumb"
                            />
                          </button>
                        </div>
                      </div>

                      {expanded && (
                        <div className="ai-mcp-server-details">
                          {server.error && <div className="ai-mcp-error">{server.error}</div>}

                          {!server.enabled ? (
                            <div className="ai-mcp-hint">
                              当前 server 已禁用，不参与状态校验，也不会向调度智能体注入工具。
                            </div>
                          ) : server.tools.length === 0 ? (
                            <div className="ai-mcp-hint">
                              当前没有可展示的工具详情。
                            </div>
                          ) : (
                            <div className="ai-mcp-tool-list">
                              {server.tools.map((tool) => {
                                const key = toolKey(server.name, tool.exposedName);
                                const toolExpanded = expandedTools[key] ?? false;

                                return (
                                  <div key={tool.exposedName} className="ai-mcp-tool-card">
                                    <button
                                      type="button"
                                      className="ai-mcp-tool-toggle"
                                      onClick={() =>
                                        setExpandedTools((current) => ({
                                          ...current,
                                          [key]: !toolExpanded,
                                        }))
                                      }
                                    >
                                      <span className="ai-mcp-chevron">
                                        {toolExpanded ? (
                                          <ChevronDown size={14} />
                                        ) : (
                                          <ChevronRight size={14} />
                                        )}
                                      </span>
                                      <span className="ai-mcp-tool-main">
                                        <span className="ai-mcp-tool-name">{tool.exposedName}</span>
                                        <span className="ai-mcp-tool-origin">
                                          原始工具名：{tool.name}
                                        </span>
                                      </span>
                                      <span className="ai-mcp-tool-pill">
                                        {taskSupportLabel(tool.taskSupport)}
                                      </span>
                                    </button>

                                    {toolExpanded && (
                                      <div className="ai-mcp-tool-details">
                                        <div className="ai-mcp-tool-desc">{tool.description}</div>
                                        <div className="ai-mcp-tool-support">
                                          调用方式：{taskSupportLabel(tool.taskSupport)}
                                        </div>
                                      </div>
                                    )}
                                  </div>
                                );
                              })}
                            </div>
                          )}
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        ) : (
          <div className="ai-mcp-loading">正在读取 MCP 状态...</div>
        )}
      </div>
    </div>
  );
}

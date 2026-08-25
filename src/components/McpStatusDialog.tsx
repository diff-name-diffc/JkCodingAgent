import { useEffect, useState } from "react";
import { ChevronDown, ChevronRight, RefreshCw, X } from "lucide-react";
import type { McpScopeKind, McpStatus, McpServerState, McpToolTaskSupport } from "../types";

function formatTimestamp(timestamp: number): string {
  if (!timestamp) return "未检查";
  return new Date(timestamp).toLocaleString();
}

function serverStateLabel(state: McpServerState): string {
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

function taskSupportLabel(taskSupport: McpToolTaskSupport): string {
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

function stateColor(state: McpServerState): string {
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

/**
 * MCP 状态弹窗，按作用域渲染：
 * - `project`：展示合并后状态（全局 ∪ 项目文件），支持启停服务器
 *   （全局条目 copy-on-write 进项目 `.jkcodingagent/mcp.json`）；
 * - `global`：所有聊天会话共享的全局注册表状态，只读展示，
 *   配置编辑入口在设置中心「MCP 服务器」页。
 */
export function McpStatusDialog({
  scope,
  status,
  checking,
  updatingServer = null,
  onRefresh,
  onToggleServerEnabled,
  onOpenSettings,
  onClose,
}: {
  scope: McpScopeKind;
  status: McpStatus | null;
  checking: boolean;
  updatingServer?: string | null;
  onRefresh: () => void;
  onToggleServerEnabled?: (serverName: string, enabled: boolean) => void;
  onOpenSettings?: () => void;
  onClose: () => void;
}) {
  const [expandedServers, setExpandedServers] = useState<Record<string, boolean>>({});
  const [expandedTools, setExpandedTools] = useState<Record<string, boolean>>({});

  const isGlobal = scope === "global";

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
            <div className="ai-mcp-title">{isGlobal ? "全局 MCP 状态" : "项目级 MCP 状态"}</div>
            <div className="ai-mcp-subtitle">
              {isGlobal
                ? "全局注册表对所有聊天与项目生效；服务器配置在设置中心「MCP 服务器」页管理"
                : "这里只支持启用或禁用 MCP server；其余配置仍通过 `.jkcodingagent/mcp.json` 管理"}
            </div>
          </div>
          <div className="ai-mcp-header-actions">
            {isGlobal && onOpenSettings && (
              <button className="ai-mcp-header-button" onClick={onOpenSettings}>
                前往设置
              </button>
            )}
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
              {isGlobal ? (
                <div className="ai-mcp-meta-row">
                  <span className="ai-mcp-meta-label">配置来源</span>
                  <span className="ai-mcp-meta-value">全局注册表（应用数据库）</span>
                </div>
              ) : (
                <>
                  <div className="ai-mcp-meta-row">
                    <span className="ai-mcp-meta-label">工作区</span>
                    <span className="ai-mcp-meta-value">{status.projectPath ?? "—"}</span>
                  </div>
                  <div className="ai-mcp-meta-row">
                    <span className="ai-mcp-meta-label">配置文件</span>
                    <span className="ai-mcp-meta-value">{status.configPath ?? "—"}</span>
                  </div>
                </>
              )}
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
                {isGlobal
                  ? "还没有配置任何全局 MCP server。请前往设置中心「MCP 服务器」页添加。"
                  : "当前没有配置任何 MCP server。请直接编辑 `.jkcodingagent/mcp.json` 后重新进入项目或点“重新检查”。"}
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

                          {!isGlobal && onToggleServerEnabled && (
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
                          )}
                        </div>
                      </div>

                      {expanded && (
                        <div className="ai-mcp-server-details">
                          {server.error && <div className="ai-mcp-error">{server.error}</div>}

                          {!server.enabled ? (
                            <div className="ai-mcp-hint">
                              当前 server 已禁用，不参与状态校验，也不会向智能体注入工具。
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

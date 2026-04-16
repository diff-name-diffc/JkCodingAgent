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
      return "#1f9d55";
    case "invalid_config":
      return "#dc2626";
    case "spawn_failed":
      return "#ea580c";
    case "connection_failed":
      return "#dc2626";
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
    <div style={styles.overlay} onClick={onClose}>
      <div style={styles.dialog} onClick={(event) => event.stopPropagation()}>
        <div style={styles.header}>
          <div>
            <div style={styles.title}>项目级 MCP 状态</div>
            <div style={styles.subtitle}>
              这里只支持启用或禁用 MCP server；其余配置仍通过 `.jkcodingagent/mcp.json` 管理
            </div>
          </div>
          <div style={styles.headerActions}>
            <button style={styles.headerBtn} onClick={onRefresh} disabled={checking}>
              <RefreshCw
                size={14}
                style={{ animation: checking ? "spin 1s linear infinite" : undefined }}
              />
              重新检查
            </button>
            <button style={styles.iconBtn} onClick={onClose} title="关闭">
              <X size={16} />
            </button>
          </div>
        </div>

        {status ? (
          <div style={styles.body}>
            <div style={styles.metaCard}>
              <div style={styles.metaRow}>
                <span style={styles.metaLabel}>工作区</span>
                <span style={styles.metaValue}>{projectPath}</span>
              </div>
              <div style={styles.metaRow}>
                <span style={styles.metaLabel}>配置文件</span>
                <span style={styles.metaValue}>{status.configPath}</span>
              </div>
              <div style={styles.metaRow}>
                <span style={styles.metaLabel}>最近检查</span>
                <span style={styles.metaValue}>{formatTimestamp(status.checkedAt)}</span>
              </div>
              <div style={styles.metaRow}>
                <span style={styles.metaLabel}>健康度</span>
                <span style={styles.metaValue}>
                  {status.healthyServerCount}/{status.enabledServerCount} 个已启用 server 可用
                  {status.serverCount !== status.enabledServerCount
                    ? `（共 ${status.serverCount} 个配置）`
                    : ""}
                </span>
              </div>
              {status.configError && <div style={styles.globalError}>{status.configError}</div>}
            </div>

            {status.servers.length === 0 ? (
              <div style={styles.emptyState}>
                当前没有配置任何 MCP server。请直接编辑 `.jkcodingagent/mcp.json`
                后重新进入项目或点“重新检查”。
              </div>
            ) : (
              <div style={styles.serverList}>
                {status.servers.map((server) => {
                  const expanded = expandedServers[server.name] ?? false;
                  const busy = updatingServer === server.name;

                  return (
                    <div key={server.name} style={styles.serverCard}>
                      <div style={styles.serverTopRow}>
                        <button
                          type="button"
                          style={styles.serverToggleBtn}
                          onClick={() =>
                            setExpandedServers((current) => ({
                              ...current,
                              [server.name]: !expanded,
                            }))
                          }
                        >
                          <span style={styles.chevronWrap}>
                            {expanded ? <ChevronDown size={16} /> : <ChevronRight size={16} />}
                          </span>
                          <span
                            style={{
                              ...styles.serverDot,
                              background: stateColor(server.state),
                            }}
                          />
                          <span style={styles.serverInfo}>
                            <span style={styles.serverTitle}>{server.name}</span>
                            <span style={styles.serverMeta}>
                              {server.transport} · {server.toolCount} 个工具 ·{" "}
                              {serverStateLabel(server.state)}
                            </span>
                          </span>
                        </button>

                        <div style={styles.serverActions}>
                          <span
                            style={{
                              ...styles.stateBadge,
                              color: stateColor(server.state),
                              borderColor: `${stateColor(server.state)}33`,
                              background: `${stateColor(server.state)}12`,
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
                            style={{
                              ...styles.switchBtn,
                              ...(server.enabled ? styles.switchBtnOn : styles.switchBtnOff),
                              ...(busy || checking ? styles.switchBtnDisabled : {}),
                            }}
                            onClick={(event) => {
                              event.stopPropagation();
                              onToggleServerEnabled(server.name, !server.enabled);
                            }}
                          >
                            <span style={styles.switchLabel}>
                              {busy ? "保存中" : server.enabled ? "启用" : "禁用"}
                            </span>
                            <span
                              style={{
                                ...styles.switchThumb,
                                transform: server.enabled ? "translateX(18px)" : "translateX(0)",
                              }}
                            />
                          </button>
                        </div>
                      </div>

                      {expanded && (
                        <div style={styles.serverDetails}>
                          {server.error && <div style={styles.serverError}>{server.error}</div>}

                          {!server.enabled ? (
                            <div style={styles.serverHint}>
                              当前 server 已禁用，不参与状态校验，也不会向调度智能体注入工具。
                            </div>
                          ) : server.tools.length === 0 ? (
                            <div style={styles.serverHint}>
                              当前没有可展示的工具详情。
                            </div>
                          ) : (
                            <div style={styles.toolList}>
                              {server.tools.map((tool) => {
                                const key = toolKey(server.name, tool.exposedName);
                                const toolExpanded = expandedTools[key] ?? false;

                                return (
                                  <div key={tool.exposedName} style={styles.toolCard}>
                                    <button
                                      type="button"
                                      style={styles.toolToggleBtn}
                                      onClick={() =>
                                        setExpandedTools((current) => ({
                                          ...current,
                                          [key]: !toolExpanded,
                                        }))
                                      }
                                    >
                                      <span style={styles.chevronWrap}>
                                        {toolExpanded ? (
                                          <ChevronDown size={14} />
                                        ) : (
                                          <ChevronRight size={14} />
                                        )}
                                      </span>
                                      <span style={styles.toolHeaderMain}>
                                        <span style={styles.toolName}>{tool.exposedName}</span>
                                        <span style={styles.toolOrigin}>
                                          原始工具名：{tool.name}
                                        </span>
                                      </span>
                                      <span style={styles.toolSupportPill}>
                                        {taskSupportLabel(tool.taskSupport)}
                                      </span>
                                    </button>

                                    {toolExpanded && (
                                      <div style={styles.toolDetails}>
                                        <div style={styles.toolDesc}>{tool.description}</div>
                                        <div style={styles.toolSupport}>
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
          <div style={styles.loadingState}>正在读取 MCP 状态...</div>
        )}
      </div>
    </div>
  );
}

const styles = {
  overlay: {
    position: "absolute" as const,
    inset: 0,
    background: "rgba(15, 23, 42, 0.32)",
    backdropFilter: "blur(8px)",
    WebkitBackdropFilter: "blur(8px)",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    zIndex: 40,
    padding: 24,
  },
  dialog: {
    width: "min(980px, 100%)",
    maxHeight: "min(780px, calc(100vh - 48px))",
    display: "flex",
    flexDirection: "column" as const,
    background:
      "linear-gradient(180deg, color-mix(in srgb, var(--bg-card) 94%, transparent), color-mix(in srgb, var(--bg-panel) 96%, transparent))",
    border: "1px solid var(--border-medium)",
    borderRadius: 18,
    boxShadow: "0 32px 90px rgba(15, 23, 42, 0.18)",
    overflow: "hidden",
  },
  header: {
    display: "flex",
    alignItems: "flex-start",
    justifyContent: "space-between",
    gap: 12,
    padding: "18px 20px",
    borderBottom: "1px solid var(--border-dim)",
  },
  title: {
    fontSize: 18,
    fontWeight: 700,
    color: "var(--text-primary)",
  },
  subtitle: {
    marginTop: 6,
    fontSize: 12,
    color: "var(--text-muted)",
    lineHeight: 1.6,
  },
  headerActions: {
    display: "flex",
    alignItems: "center",
    gap: 8,
    flexShrink: 0,
  },
  headerBtn: {
    display: "inline-flex",
    alignItems: "center",
    gap: 6,
    padding: "8px 12px",
    borderRadius: 999,
    border: "1px solid var(--border-medium)",
    background: "var(--bg-panel)",
    color: "var(--text-secondary)",
    cursor: "pointer",
    fontSize: 12,
  },
  iconBtn: {
    width: 34,
    height: 34,
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    borderRadius: 999,
    border: "1px solid var(--border-medium)",
    background: "var(--bg-panel)",
    color: "var(--text-secondary)",
    cursor: "pointer",
  },
  body: {
    padding: 20,
    overflowY: "auto" as const,
    display: "flex",
    flexDirection: "column" as const,
    gap: 16,
  },
  metaCard: {
    display: "flex",
    flexDirection: "column" as const,
    gap: 10,
    padding: 16,
    borderRadius: 14,
    border: "1px solid var(--border-medium)",
    background: "var(--bg-panel)",
  },
  metaRow: {
    display: "flex",
    gap: 12,
    alignItems: "flex-start",
  },
  metaLabel: {
    width: 72,
    flexShrink: 0,
    fontSize: 12,
    color: "var(--text-muted)",
  },
  metaValue: {
    fontSize: 13,
    color: "var(--text-primary)",
    wordBreak: "break-all" as const,
  },
  globalError: {
    padding: "10px 12px",
    borderRadius: 10,
    background: "rgba(220, 38, 38, 0.08)",
    color: "#b91c1c",
    fontSize: 12,
    lineHeight: 1.6,
  },
  emptyState: {
    padding: 18,
    borderRadius: 14,
    border: "1px dashed var(--border-medium)",
    color: "var(--text-muted)",
    fontSize: 13,
    lineHeight: 1.7,
    background: "var(--bg-panel)",
  },
  loadingState: {
    padding: 24,
    color: "var(--text-muted)",
    fontSize: 13,
  },
  serverList: {
    display: "flex",
    flexDirection: "column" as const,
    gap: 14,
  },
  serverCard: {
    display: "flex",
    flexDirection: "column" as const,
    borderRadius: 14,
    border: "1px solid var(--border-medium)",
    background:
      "linear-gradient(180deg, color-mix(in srgb, var(--bg-card) 92%, transparent), color-mix(in srgb, var(--bg-panel) 96%, transparent))",
    overflow: "hidden",
  },
  serverTopRow: {
    display: "flex",
    alignItems: "stretch",
    justifyContent: "space-between",
    gap: 12,
    padding: 14,
  },
  serverToggleBtn: {
    flex: 1,
    minWidth: 0,
    display: "flex",
    alignItems: "flex-start",
    gap: 10,
    padding: 0,
    border: "none",
    background: "transparent",
    cursor: "pointer",
    color: "inherit",
    textAlign: "left" as const,
  },
  chevronWrap: {
    width: 18,
    height: 18,
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    color: "var(--text-muted)",
    flexShrink: 0,
    marginTop: 2,
  },
  serverDot: {
    width: 10,
    height: 10,
    borderRadius: 999,
    marginTop: 5,
    flexShrink: 0,
  },
  serverInfo: {
    minWidth: 0,
    display: "flex",
    flexDirection: "column" as const,
    gap: 4,
  },
  serverTitle: {
    fontSize: 15,
    fontWeight: 700,
    color: "var(--text-primary)",
  },
  serverMeta: {
    fontSize: 12,
    color: "var(--text-muted)",
  },
  serverActions: {
    display: "flex",
    alignItems: "center",
    gap: 10,
    flexShrink: 0,
  },
  stateBadge: {
    display: "inline-flex",
    alignItems: "center",
    padding: "6px 10px",
    borderRadius: 999,
    border: "1px solid transparent",
    fontSize: 12,
    fontWeight: 600,
    whiteSpace: "nowrap" as const,
  },
  switchBtn: {
    position: "relative" as const,
    minWidth: 72,
    height: 30,
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: 8,
    padding: "0 8px",
    borderRadius: 999,
    border: "1px solid transparent",
    cursor: "pointer",
    transition: "all 120ms ease",
  },
  switchBtnOn: {
    background: "rgba(31, 157, 85, 0.14)",
    borderColor: "rgba(31, 157, 85, 0.22)",
    color: "#166534",
  },
  switchBtnOff: {
    background: "rgba(148, 163, 184, 0.12)",
    borderColor: "rgba(148, 163, 184, 0.2)",
    color: "var(--text-muted)",
  },
  switchBtnDisabled: {
    opacity: 0.6,
    cursor: "not-allowed",
  },
  switchLabel: {
    fontSize: 11,
    fontWeight: 700,
    paddingRight: 22,
  },
  switchThumb: {
    position: "absolute" as const,
    right: 8,
    width: 18,
    height: 18,
    borderRadius: 999,
    background: "#fff",
    boxShadow: "0 2px 6px rgba(15, 23, 42, 0.18)",
    transition: "transform 120ms ease",
  },
  serverDetails: {
    display: "flex",
    flexDirection: "column" as const,
    gap: 12,
    padding: "0 14px 14px 14px",
    borderTop: "1px solid var(--border-dim)",
    background: "color-mix(in srgb, var(--bg-panel) 92%, transparent)",
  },
  serverError: {
    marginTop: 12,
    padding: "10px 12px",
    borderRadius: 10,
    background: "rgba(220, 38, 38, 0.08)",
    color: "#b91c1c",
    fontSize: 12,
    lineHeight: 1.6,
    whiteSpace: "pre-wrap" as const,
  },
  serverHint: {
    marginTop: 12,
    padding: "12px 14px",
    borderRadius: 10,
    border: "1px dashed var(--border-medium)",
    color: "var(--text-muted)",
    fontSize: 12,
    lineHeight: 1.6,
    background: "color-mix(in srgb, var(--bg-card) 90%, transparent)",
  },
  toolList: {
    display: "flex",
    flexDirection: "column" as const,
    gap: 10,
    marginTop: 12,
  },
  toolCard: {
    borderRadius: 12,
    border: "1px solid var(--border-dim)",
    background: "color-mix(in srgb, var(--bg-card) 90%, transparent)",
    overflow: "hidden",
  },
  toolToggleBtn: {
    width: "100%",
    display: "flex",
    alignItems: "flex-start",
    gap: 10,
    padding: 12,
    border: "none",
    background: "transparent",
    cursor: "pointer",
    color: "inherit",
    textAlign: "left" as const,
  },
  toolHeaderMain: {
    flex: 1,
    minWidth: 0,
    display: "flex",
    flexDirection: "column" as const,
    gap: 4,
  },
  toolName: {
    fontFamily: "var(--font-mono)",
    fontSize: 12,
    color: "var(--text-primary)",
    wordBreak: "break-all" as const,
  },
  toolOrigin: {
    fontSize: 11,
    color: "var(--text-hint)",
    wordBreak: "break-all" as const,
  },
  toolSupportPill: {
    display: "inline-flex",
    alignItems: "center",
    padding: "4px 8px",
    borderRadius: 999,
    background: "var(--bg-panel)",
    border: "1px solid var(--border-medium)",
    color: "var(--text-muted)",
    fontSize: 11,
    whiteSpace: "nowrap" as const,
  },
  toolDetails: {
    display: "flex",
    flexDirection: "column" as const,
    gap: 8,
    padding: "0 12px 12px 40px",
    borderTop: "1px solid var(--border-dim)",
  },
  toolDesc: {
    fontSize: 12,
    color: "var(--text-secondary)",
    lineHeight: 1.7,
    whiteSpace: "pre-wrap" as const,
  },
  toolSupport: {
    fontSize: 11,
    color: "var(--text-muted)",
  },
};

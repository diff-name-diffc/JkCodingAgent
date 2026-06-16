import { useCallback, useEffect, useState } from "react";
import type React from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { Check, FolderOpen, Plus, RefreshCw, Trash2 } from "lucide-react";
import type { AgentContext, SshAuditLog, SshServerConfig, SshToolsConfig } from "../../../types";
import s from "../../../styles";

const EMPTY_SERVER: SshServerConfig = {
  id: "",
  enabled: true,
  host: "",
  port: 22,
  username: "",
  password: "",
  authMethod: "password",
  privateKeyPath: "",
  privateKeyPassphrase: "",
  description: "",
  tags: [],
  defaultTimeoutSecs: 30,
  maxOutputBytes: 65536,
};

export function SshToolPanel({ projectPath }: { projectPath?: string }) {
  const [context, setContext] = useState<AgentContext>(projectPath ? "project" : "chat");
  const [workspacePath, setWorkspacePath] = useState("");
  const [config, setConfig] = useState<SshToolsConfig>({ servers: [] });
  const [audit, setAudit] = useState<SshAuditLog>({ records: [] });
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [testingId, setTestingId] = useState<string | null>(null);
  const [testResult, setTestResult] = useState<Record<string, string>>({});

  const loadConfig = useCallback(async () => {
    if (context === "project" && !projectPath) {
      setError("当前没有项目路径，请切换到聊天 SSH 配置。");
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const resolvedWorkspace = await invoke<string>("aha_resolve_ssh_workspace", {
        context,
        projectPath: context === "project" ? projectPath : null,
      });
      const [loadedConfig, loadedAudit] = await Promise.all([
        invoke<SshToolsConfig>("ssh_tool_load_config", { projectPath: resolvedWorkspace }),
        invoke<SshAuditLog>("ssh_tool_load_audit", { projectPath: resolvedWorkspace }),
      ]);
      setWorkspacePath(resolvedWorkspace);
      setConfig(loadedConfig);
      setAudit(loadedAudit);
      setTestResult({});
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, [context, projectPath]);

  useEffect(() => {
    loadConfig();
  }, [loadConfig]);

  function switchContext(next: AgentContext) {
    setContext(next);
    setSaved(false);
    setError(null);
  }

  function updateServer(index: number, updater: (server: SshServerConfig) => SshServerConfig) {
    setConfig((prev) => ({
      servers: prev.servers.map((server, i) => (i === index ? updater(server) : server)),
    }));
  }

  function addServer() {
    setConfig((prev) => ({
      servers: [...prev.servers, { ...EMPTY_SERVER, id: nextServerId(prev.servers) }],
    }));
  }

  function removeServer(index: number) {
    setConfig((prev) => ({ servers: prev.servers.filter((_, i) => i !== index) }));
  }

  async function saveConfig() {
    if (!workspacePath) return;
    setSaving(true);
    setSaved(false);
    setError(null);
    try {
      const savedConfig = await invoke<SshToolsConfig>("ssh_tool_save_config", {
        projectPath: workspacePath,
        config,
      });
      setConfig(savedConfig);
      setSaved(true);
      window.setTimeout(() => setSaved(false), 2000);
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  }

  async function testConnection(server: SshServerConfig) {
    const resultKey = server.id || "__draft__";
    if (!server.id.trim()) return;
    setTestingId(server.id);
    setTestResult((prev) => ({ ...prev, [resultKey]: "" }));
    try {
      const message = await invoke<string>("ssh_tool_test_server_config", { server });
      setTestResult((prev) => ({ ...prev, [resultKey]: message }));
    } catch (err) {
      setTestResult((prev) => ({ ...prev, [resultKey]: String(err) }));
    } finally {
      setTestingId(null);
    }
  }

  return (
    <>
      <div style={s.ahaBody}>
        <div style={s.ahaContent}>
          <div style={s.ahaSection}>
            <div style={s.ahaSectionHeader}>
              <div>
                <div style={s.ahaSectionTitle}>SSH 工具配置</div>
                <div style={s.ahaSectionDescription}>
                  项目和聊天分别使用自己的本地 SSH 环境。配置与审计文件位于
                  <span style={{ fontFamily: "var(--font-mono)" }}>
                    {" "}
                    .jkcodingagent/local_env/ssh
                  </span>
                  。
                </div>
              </div>
              <div style={s.ahaActionRow}>
                <button
                  type="button"
                  style={contextButtonStyle(context === "project", !projectPath)}
                  disabled={!projectPath}
                  onClick={() => switchContext("project")}
                >
                  项目
                </button>
                <button
                  type="button"
                  style={contextButtonStyle(context === "chat", false)}
                  onClick={() => switchContext("chat")}
                >
                  聊天
                </button>
                <button
                  type="button"
                  style={s.ahaGhostButton}
                  onClick={loadConfig}
                  disabled={loading}
                >
                  <RefreshCw size={13} />
                  刷新
                </button>
                <button type="button" style={s.ahaGhostButton} onClick={addServer}>
                  <Plus size={13} />
                  添加服务器
                </button>
              </div>
            </div>

            {workspacePath && (
              <div style={s.ahaHint}>
                当前工作区：
                <span style={{ fontFamily: "var(--font-mono)" }}>{workspacePath}</span>
              </div>
            )}

            {loading ? (
              <div style={s.ahaHint}>加载中...</div>
            ) : config.servers.length === 0 ? (
              <div style={s.ahaHint}>还没有配置 SSH server。</div>
            ) : (
              <div style={s.ahaGrid}>
                {config.servers.map((server, index) => (
                  <ServerEditor
                    key={`${server.id}-${index}`}
                    server={server}
                    index={index}
                    testingId={testingId}
                    testResult={testResult[server.id || "__draft__"]}
                    onUpdate={updateServer}
                    onRemove={removeServer}
                    onTest={testConnection}
                  />
                ))}
              </div>
            )}
          </div>

          <div style={s.ahaSection}>
            <div style={s.ahaSectionHeader}>
              <div>
                <div style={s.ahaSectionTitle}>SSH 审计记录</div>
                <div style={s.ahaSectionDescription}>
                  保留最先的 50 条命令记录，包含会话、命令、退出码和执行结果。
                </div>
              </div>
              <span style={s.ahaHint}>{audit.records.length}/50</span>
            </div>
            {audit.records.length === 0 ? (
              <div style={s.ahaHint}>暂无审计记录。</div>
            ) : (
              <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
                {audit.records.map((record, index) => (
                  <div key={`${record.createdAt}-${index}`} style={s.ahaProvider}>
                    <div style={s.ahaProviderHeader}>
                      <div style={s.ahaProviderTitleWrap}>
                        <span style={s.ahaProviderTitle}>{record.serverId}</span>
                        <span style={s.ahaProviderSummary}>{record.sessionId}</span>
                        {record.interactiveBlocked ? (
                          <span style={interactiveBadgeStyle}>交互阻塞</span>
                        ) : null}
                      </div>
                      <span style={s.ahaHint}>{record.createdAt}</span>
                    </div>
                    <pre style={auditPreStyle}>{record.command}</pre>
                    <div style={s.ahaHint}>
                      {record.interactiveBlocked
                        ? "exit=交互阻塞(已中止)"
                        : `exit=${record.exitCode ?? "error"}`}
                      {" · duration="}
                      {record.durationMs ?? "-"}ms
                      {record.truncated ? " · truncated" : ""}
                      {record.error ? ` · ${record.error}` : ""}
                    </div>
                    {record.interactiveBlocked && record.stderr.trim() ? (
                      <pre style={{ ...auditPreStyle, ...interactiveDetailStyle }}>
                        {record.stderr}
                      </pre>
                    ) : null}
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>
      </div>

      <div style={s.settingsFooter}>
        {error && (
          <span style={{ ...s.ahaFeedback, color: "var(--danger)", marginRight: "auto" }}>
            {error}
          </span>
        )}
        {saved && (
          <span style={{ ...s.ahaFeedback, color: "var(--success)", marginRight: "auto" }}>
            已保存
          </span>
        )}
        <button
          type="button"
          style={{ ...s.modalSaveBtn, opacity: saving || !workspacePath ? 0.5 : 1 }}
          onClick={saveConfig}
          disabled={saving || !workspacePath}
        >
          {saving ? "保存中..." : "保存 SSH 配置"}
        </button>
      </div>
    </>
  );
}

function ServerEditor({
  server,
  index,
  testingId,
  testResult,
  onUpdate,
  onRemove,
  onTest,
}: {
  server: SshServerConfig;
  index: number;
  testingId: string | null;
  testResult?: string;
  onUpdate: (index: number, updater: (server: SshServerConfig) => SshServerConfig) => void;
  onRemove: (index: number) => void;
  onTest: (server: SshServerConfig) => void;
}) {
  return (
    <div style={s.ahaProvider}>
      <div style={s.ahaProviderHeader}>
        <label style={s.ahaToggleRow}>
          <input
            type="checkbox"
            checked={server.enabled}
            onChange={(event) =>
              onUpdate(index, (draft) => ({ ...draft, enabled: event.target.checked }))
            }
          />
          <span style={s.ahaProviderTitle}>{server.id || `server-${index + 1}`}</span>
        </label>
        <div style={s.ahaActionRow}>
          <button
            type="button"
            style={s.ahaGhostButton}
            onClick={() => onTest(server)}
            disabled={testingId === server.id || !server.id.trim()}
          >
            <Check size={13} />
            {testingId === server.id ? "测试中..." : "测试连接"}
          </button>
          <button
            type="button"
            style={{ ...s.ahaGhostButton, color: "var(--danger)" }}
            onClick={() => onRemove(index)}
          >
            <Trash2 size={13} />
            删除
          </button>
        </div>
      </div>

      <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 10 }}>
        <Field label="Server ID">
          <input
            style={s.ahaInput}
            value={server.id}
            placeholder="prod-web-1"
            onChange={(event) =>
              onUpdate(index, (draft) => ({ ...draft, id: event.target.value.toLowerCase() }))
            }
          />
        </Field>
        <Field label="描述">
          <input
            style={{ ...s.ahaInput, fontFamily: "inherit" }}
            value={server.description}
            placeholder="生产 Web 节点"
            onChange={(event) =>
              onUpdate(index, (draft) => ({ ...draft, description: event.target.value }))
            }
          />
        </Field>
        <Field label="Host">
          <input
            style={s.ahaInput}
            value={server.host}
            placeholder="10.0.1.12"
            onChange={(event) =>
              onUpdate(index, (draft) => ({ ...draft, host: event.target.value }))
            }
          />
        </Field>
        <Field label="Port">
          <input
            style={s.ahaInput}
            type="number"
            min={1}
            max={65535}
            value={server.port}
            onChange={(event) =>
              onUpdate(index, (draft) => ({ ...draft, port: Number(event.target.value) || 22 }))
            }
          />
        </Field>
        <Field label="Username">
          <input
            style={s.ahaInput}
            value={server.username}
            onChange={(event) =>
              onUpdate(index, (draft) => ({ ...draft, username: event.target.value }))
            }
          />
        </Field>
        <Field label="Timeout 秒">
          <input
            style={s.ahaInput}
            type="number"
            min={1}
            max={300}
            value={server.defaultTimeoutSecs}
            onChange={(event) =>
              onUpdate(index, (draft) => ({
                ...draft,
                defaultTimeoutSecs: Number(event.target.value) || 30,
              }))
            }
          />
        </Field>
        <Field label="最大输出字节">
          <input
            style={s.ahaInput}
            type="number"
            min={1024}
            max={1048576}
            value={server.maxOutputBytes}
            onChange={(event) =>
              onUpdate(index, (draft) => ({
                ...draft,
                maxOutputBytes: Number(event.target.value) || 65536,
              }))
            }
          />
        </Field>
      </div>

      <AuthMethodEditor server={server} index={index} onUpdate={onUpdate} />

      <Field label="标签">
        <input
          style={s.ahaInput}
          value={server.tags.join(", ")}
          placeholder="prod, web"
          onChange={(event) =>
            onUpdate(index, (draft) => ({
              ...draft,
              tags: event.target.value
                .split(",")
                .map((tag) => tag.trim())
                .filter(Boolean),
            }))
          }
        />
      </Field>

      {testResult && (
        <div
          style={{
            ...s.ahaFeedback,
            color: testResult.includes("成功") ? "var(--success)" : "var(--danger)",
          }}
        >
          {testResult}
        </div>
      )}
    </div>
  );
}

function AuthMethodEditor({
  server,
  index,
  onUpdate,
}: {
  server: SshServerConfig;
  index: number;
  onUpdate: (index: number, updater: (server: SshServerConfig) => SshServerConfig) => void;
}) {
  async function pickKeyFile() {
    const selected = await openDialog({
      directory: false,
      multiple: false,
      title: "选择 SSH 私钥文件（如 ~/.ssh/id_rsa、id_ed25519）",
    });
    if (typeof selected === "string" && selected.length > 0) {
      onUpdate(index, (draft) => ({ ...draft, privateKeyPath: selected }));
    }
  }

  return (
    <>
      <Field label="认证方式">
        <div style={{ display: "flex", gap: 8 }}>
          <button
            type="button"
            style={authButtonStyle(server.authMethod === "password")}
            onClick={() => onUpdate(index, (draft) => ({ ...draft, authMethod: "password" }))}
          >
            密码
          </button>
          <button
            type="button"
            style={authButtonStyle(server.authMethod === "key")}
            onClick={() => onUpdate(index, (draft) => ({ ...draft, authMethod: "key" }))}
          >
            私钥文件
          </button>
        </div>
      </Field>

      {server.authMethod === "password" ? (
        <Field label="Password">
          <input
            style={s.ahaInput}
            type="password"
            value={server.password}
            onChange={(event) =>
              onUpdate(index, (draft) => ({ ...draft, password: event.target.value }))
            }
          />
        </Field>
      ) : (
        <>
          <Field label="私钥文件路径">
            <div style={keyFileRowStyle}>
              <input
                style={{ ...s.ahaInput, flex: 1 }}
                value={server.privateKeyPath}
                placeholder="~/.ssh/id_rsa"
                onChange={(event) =>
                  onUpdate(index, (draft) => ({ ...draft, privateKeyPath: event.target.value }))
                }
              />
              <button type="button" style={s.ahaGhostButton} onClick={pickKeyFile}>
                <FolderOpen size={13} />
                选择
              </button>
            </div>
          </Field>
          <Field label="私钥口令（可选，加密私钥时填写）">
            <input
              style={s.ahaInput}
              type="password"
              value={server.privateKeyPassphrase}
              onChange={(event) =>
                onUpdate(index, (draft) => ({
                  ...draft,
                  privateKeyPassphrase: event.target.value,
                }))
              }
            />
          </Field>
        </>
      )}
    </>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label style={s.ahaField}>
      <span style={s.ahaLabel}>{label}</span>
      {children}
    </label>
  );
}

function nextServerId(servers: SshServerConfig[]) {
  let index = servers.length + 1;
  let id = `server-${index}`;
  const used = new Set(servers.map((server) => server.id));
  while (used.has(id)) {
    index += 1;
    id = `server-${index}`;
  }
  return id;
}

function contextButtonStyle(selected: boolean, disabled: boolean): React.CSSProperties {
  return {
    ...s.ahaGhostButton,
    background: selected ? "var(--bg-hover)" : "var(--bg-card)",
    color: selected ? "var(--text-primary)" : "var(--text-secondary)",
    opacity: disabled ? 0.5 : 1,
  };
}

function authButtonStyle(selected: boolean): React.CSSProperties {
  return {
    ...s.ahaGhostButton,
    background: selected ? "var(--bg-hover)" : "var(--bg-card)",
    color: selected ? "var(--text-primary)" : "var(--text-secondary)",
    borderColor: selected ? "var(--accent)" : "var(--border-dim)",
  };
}

const keyFileRowStyle: React.CSSProperties = {
  display: "flex",
  gap: 8,
  alignItems: "stretch",
};

const auditPreStyle: React.CSSProperties = {
  margin: 0,
  padding: "8px 10px",
  borderRadius: 8,
  border: "1px solid var(--border-dim)",
  background: "var(--bg-subtle)",
  color: "var(--text-primary)",
  fontFamily: "var(--font-mono)",
  fontSize: 11.5,
  lineHeight: 1.45,
  whiteSpace: "pre-wrap",
  wordBreak: "break-word",
};

const interactiveBadgeStyle: React.CSSProperties = {
  fontSize: 10.5,
  fontWeight: 600,
  color: "var(--warning)",
  border: "1px solid var(--warning)",
  borderRadius: 6,
  padding: "0 6px",
  lineHeight: 1.5,
};

const interactiveDetailStyle: React.CSSProperties = {
  maxHeight: 160,
  overflow: "auto",
  color: "var(--text-secondary)",
  fontSize: 11,
  borderColor: "var(--warning)",
};

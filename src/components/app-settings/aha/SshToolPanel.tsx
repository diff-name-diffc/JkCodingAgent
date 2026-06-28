import { useCallback, useEffect, useState } from "react";
import type React from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { Check, ChevronDown, FolderOpen, Plus, RefreshCw, RotateCcw, Trash2 } from "lucide-react";
import type {
  AgentContext,
  AhaSettingsV2,
  SshAuditLog,
  SshReviewConfig,
  SshServerConfig,
  SshToolsConfig,
} from "../../../types";
import s from "../../../styles";
import { SshAuditRecordList } from "./SshAuditRecordList";

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
  reviewEnabled: true,
  defaultTimeoutSecs: 30,
  maxOutputBytes: 65536,
};

/// 命令安全审查 AI 默认系统提示词（须与后端 DEFAULT_REVIEW_SYSTEM_PROMPT 保持一致）。
const DEFAULT_REVIEW_PROMPT =
  "你是命令安全审查员。依据用户的任务、当前意图、目标环境信息和待执行命令，判断该命令是否可安全执行。\n\n" +
  "判定原则：\n" +
  "- 拒绝：不可逆或高危操作，如删除/覆盖系统文件或关键数据（rm -rf 指向根目录或家目录、mkfs、dd 覆写块设备、清空数据库/表）、关机重启、提权后执行破坏性操作、fork 炸弹/资源耗尽、关闭防火墙或清空路由、向外部批量外传敏感数据。\n" +
  "- 允许：常规只读巡检、查询状态、在用户明确指定目录内的受控写操作。\n" +
  "- 必须结合「任务」「意图」和「目标环境」综合判断：同一命令在不同上下文风险不同（如 rm 清理临时目录可允许，针对根目录或用户家目录则拒绝）。无法确认影响范围或意图不明时，倾向拒绝。\n\n" +
  "输出格式：仅一行。`ALLOW` 表示允许；`DENY: <简短中文原因>` 表示拒绝。不要输出任何多余内容。";

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
  const [expandedAudit, setExpandedAudit] = useState<string | null>(null);
  // 每台服务器默认折叠，仅在用户展开或新增时展开其详细配置。
  const [expandedServers, setExpandedServers] = useState<Set<number>>(new Set());
  // 审查 AI 配置（全局，存于 Aha 设置；保存时需整体回写以保留其他字段）。
  const [ahaSettings, setAhaSettings] = useState<AhaSettingsV2 | null>(null);
  const [reviewSaving, setReviewSaving] = useState(false);
  const [reviewSaved, setReviewSaved] = useState(false);
  const [reviewError, setReviewError] = useState<string | null>(null);
  const [reviewTesting, setReviewTesting] = useState(false);
  const [reviewFeedback, setReviewFeedback] = useState<string | null>(null);

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

  // 审查 AI 配置全局共享，独立于项目/聊天工作区加载一次。
  useEffect(() => {
    invoke<AhaSettingsV2>("aha_get_settings_v2")
      .then((settings) => setAhaSettings(settings))
      .catch(() => setAhaSettings(null));
  }, []);

  const review: SshReviewConfig = ahaSettings?.review ?? {
    modelConfig: { url: "", apiKey: "", model: "", active: true },
    systemPrompt: DEFAULT_REVIEW_PROMPT,
  };

  function updateReview(updater: (draft: SshReviewConfig) => SshReviewConfig) {
    setAhaSettings((prev) => {
      const base: AhaSettingsV2 = prev ?? {
        shared: {
          visionModelConfigs: [],
          imageModelConfigs: [],
          imageEditModelConfigs: [],
          asrModelConfigs: [],
          ttsModelConfigs: [],
          embeddingModelConfigs: [],
        },
        project: { chatModelConfigs: [], summaryModelConfigs: [], allowedTools: [] },
        chat: { chatModelConfigs: [], summaryModelConfigs: [], allowedTools: [] },
        autoApproveDispatch: false,
        contextDebug: false,
        review,
      };
      return { ...base, review: updater(base.review) };
    });
  }

  async function saveReview() {
    if (!ahaSettings) return;
    setReviewSaving(true);
    setReviewError(null);
    try {
      const result = await invoke<AhaSettingsV2>("aha_save_settings_v2", { settings: ahaSettings });
      setAhaSettings(result);
      setReviewSaved(true);
      window.setTimeout(() => setReviewSaved(false), 2000);
    } catch (err) {
      setReviewError(String(err));
    } finally {
      setReviewSaving(false);
    }
  }

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

  function toggleServer(index: number) {
    setExpandedServers((prev) => {
      const next = new Set(prev);
      if (next.has(index)) next.delete(index);
      else next.add(index);
      return next;
    });
  }

  function addServer() {
    const newIndex = config.servers.length;
    setConfig((prev) => ({
      servers: [...prev.servers, { ...EMPTY_SERVER, id: nextServerId(prev.servers) }],
    }));
    // 新增的服务器自动展开，方便立即填写。
    setExpandedServers((prev) => new Set(prev).add(newIndex));
  }

  function removeServer(index: number) {
    setConfig((prev) => ({ servers: prev.servers.filter((_, i) => i !== index) }));
    // 删除后保持其余服务器展开状态与列表下标一致。
    setExpandedServers((prev) => {
      const next = new Set<number>();
      for (const i of prev) {
        if (i < index) next.add(i);
        else if (i > index) next.add(i - 1);
      }
      return next;
    });
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
          <ReviewAiSection
            review={review}
            testing={reviewTesting}
            feedback={reviewFeedback}
            saving={reviewSaving}
            saved={reviewSaved}
            error={reviewError}
            onChange={updateReview}
            onSave={saveReview}
            onTest={async () => {
              if (reviewTesting) return;
              setReviewTesting(true);
              setReviewFeedback(null);
              try {
                const message = await invoke<string>("dispatcher_test_model", {
                  kind: "review",
                  config: review.modelConfig,
                });
                setReviewFeedback(message);
              } catch (err) {
                setReviewFeedback(String(err));
              } finally {
                setReviewTesting(false);
              }
            }}
          />

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
                    expanded={expandedServers.has(index)}
                    onToggleExpand={() => toggleServer(index)}
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
                  保留最近的 100 条命令记录，包含审查结论、会话、命令和执行结果。
                </div>
              </div>
              <span style={s.ahaHint}>{audit.records.length}/100</span>
            </div>
            <SshAuditRecordList
              records={audit.records}
              expandedAudit={expandedAudit}
              onExpandedAuditChange={setExpandedAudit}
            />
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
  expanded,
  onToggleExpand,
  testingId,
  testResult,
  onUpdate,
  onRemove,
  onTest,
}: {
  server: SshServerConfig;
  index: number;
  expanded: boolean;
  onToggleExpand: () => void;
  testingId: string | null;
  testResult?: string;
  onUpdate: (index: number, updater: (server: SshServerConfig) => SshServerConfig) => void;
  onRemove: (index: number) => void;
  onTest: (server: SshServerConfig) => void;
}) {
  return (
    <div style={s.ahaProvider}>
      <div style={s.ahaProviderHeader}>
        <div style={serverHeaderLeftStyle}>
          <input
            type="checkbox"
            checked={server.enabled}
            title={server.enabled ? "已启用" : "已停用"}
            style={{ accentColor: "var(--accent)", cursor: "pointer", flexShrink: 0 }}
            onChange={(event) =>
              onUpdate(index, (draft) => ({ ...draft, enabled: event.target.checked }))
            }
          />
          <button type="button" style={s.ahaProviderTitleButton} onClick={onToggleExpand}>
            <ChevronDown
              size={14}
              style={{
                transform: expanded ? "rotate(0deg)" : "rotate(-90deg)",
                transition: "transform 0.15s",
                flexShrink: 0,
              }}
            />
            <span style={s.ahaProviderTitleWrap}>
              <span style={s.ahaProviderTitle}>{server.id || `server-${index + 1}`}</span>
              <span style={s.ahaProviderSummary}>{serverSummary(server)}</span>
            </span>
          </button>
        </div>
        <div style={s.ahaProviderActions}>
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

      {expanded && (
        <>
          <label
            style={{ ...s.ahaToggleRow, marginLeft: 22 }}
            title="开启后，每条命令执行前先经审查 AI 评估安全性"
          >
            <input
              type="checkbox"
              checked={server.reviewEnabled}
              onChange={(event) =>
                onUpdate(index, (draft) => ({ ...draft, reviewEnabled: event.target.checked }))
              }
            />
            <span style={{ ...s.ahaProviderSummary, fontSize: 11 }}>执行前审查</span>
          </label>

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
        </>
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

function ReviewAiSection({
  review,
  testing,
  feedback,
  saving,
  saved,
  error,
  onChange,
  onSave,
  onTest,
}: {
  review: SshReviewConfig;
  testing: boolean;
  feedback: string | null;
  saving: boolean;
  saved: boolean;
  error: string | null;
  onChange: (updater: (draft: SshReviewConfig) => SshReviewConfig) => void;
  onSave: () => void;
  onTest: () => void;
}) {
  const [expanded, setExpanded] = useState(true);
  return (
    <div style={s.ahaSection}>
      <div style={s.ahaSectionHeader}>
        <button type="button" style={s.ahaCollapsibleTitle} onClick={() => setExpanded((v) => !v)}>
          <ChevronDown
            size={14}
            style={{
              transform: expanded ? "rotate(0deg)" : "rotate(-90deg)",
              transition: "transform 0.15s",
              flexShrink: 0,
              marginTop: 2,
            }}
          />
          <span>
            <div style={s.ahaSectionTitle}>命令审查 AI</div>
            <div style={s.ahaSectionDescription}>
              配置后，SSH 与本地 local_zsh 命令执行前会交由该 OpenAI
              兼容模型，结合意图/任务/目标环境/命令做安全审查；审查异常或判定不通过将阻断执行。该配置全局共享（项目与聊天通用）。
            </div>
          </span>
        </button>
        <div style={s.ahaActionRow}>
          <button
            type="button"
            style={s.ahaGhostButton}
            onClick={onTest}
            disabled={testing || !review.modelConfig.model.trim()}
          >
            <Check size={13} />
            {testing ? "测试中..." : "测试审查模型"}
          </button>
          <button type="button" style={s.ahaGhostButton} onClick={onSave} disabled={saving}>
            {saving ? "保存中..." : "保存审查配置"}
          </button>
        </div>
      </div>

      {expanded && (
        <>
          <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 10 }}>
            <Field label="审查模型 URL（OpenAI 兼容）">
              <input
                style={s.ahaInput}
                value={review.modelConfig.url}
                placeholder="https://api.example.com/v1"
                onChange={(event) =>
                  onChange((draft) => ({
                    ...draft,
                    modelConfig: { ...draft.modelConfig, url: event.target.value },
                  }))
                }
              />
            </Field>
            <Field label="API Key">
              <input
                style={s.ahaInput}
                type="password"
                value={review.modelConfig.apiKey}
                onChange={(event) =>
                  onChange((draft) => ({
                    ...draft,
                    modelConfig: { ...draft.modelConfig, apiKey: event.target.value },
                  }))
                }
              />
            </Field>
            <Field label="模型名">
              <input
                style={s.ahaInput}
                value={review.modelConfig.model}
                placeholder="gpt-4o-mini"
                onChange={(event) =>
                  onChange((draft) => ({
                    ...draft,
                    modelConfig: { ...draft.modelConfig, model: event.target.value },
                  }))
                }
              />
            </Field>
          </div>

          <Field label="系统提示词">
            <textarea
              style={{ ...s.ahaInput, minHeight: 150, resize: "vertical", fontFamily: "inherit" }}
              value={review.systemPrompt}
              onChange={(event) =>
                onChange((draft) => ({ ...draft, systemPrompt: event.target.value }))
              }
            />
            <button
              type="button"
              style={{ ...s.ahaGhostButton, marginTop: 6 }}
              onClick={() =>
                onChange((draft) => ({ ...draft, systemPrompt: DEFAULT_REVIEW_PROMPT }))
              }
            >
              <RotateCcw size={13} />
              恢复默认提示词
            </button>
          </Field>

          {feedback && (
            <div
              style={{
                ...s.ahaFeedback,
                color:
                  feedback.includes("ok") || feedback.includes("成功")
                    ? "var(--success)"
                    : "var(--danger)",
              }}
            >
              {feedback}
            </div>
          )}
          {error && <span style={{ ...s.ahaFeedback, color: "var(--danger)" }}>{error}</span>}
          {saved && (
            <span style={{ ...s.ahaFeedback, color: "var(--success)" }}>审查配置已保存</span>
          )}
        </>
      )}
    </div>
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

/// 折叠时在服务器标题右侧展示的连接摘要，便于在不展开的情况下辨识目标主机。
function serverSummary(server: SshServerConfig): string {
  if (server.host.trim()) {
    const auth = server.username.trim() ? `${server.username}@` : "";
    const port = server.port && server.port !== 22 ? `:${server.port}` : "";
    return `${auth}${server.host}${port}`;
  }
  return server.description.trim() || "未配置";
}

const serverHeaderLeftStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 10,
  minWidth: 0,
  flex: 1,
};

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

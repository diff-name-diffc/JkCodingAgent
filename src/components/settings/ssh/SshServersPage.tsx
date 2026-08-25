import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Download, Plus, RefreshCw, RotateCcw, Server } from "lucide-react";
import type { SshAuditLog, SshServerConfig, SshToolsConfig } from "../../../types";
import { SshAuditRecordList } from "../../app-settings/aha/SshAuditRecordList";
import { ConfirmDialog } from "../ConfirmDialog";
import { EmptyState } from "../EmptyState";
import { FieldLabel } from "../FieldLabel";
import { Section } from "../Section";
import { toast } from "../toast";
import { useAhaSettings } from "../use-aha-settings";
import {
  loadSshTestRecords,
  recordSshTest,
  type ProviderTestRecord,
} from "../providers/provider-prefs";
import { SshImportDialog } from "./SshImportDialog";
import { SshServerCard } from "./SshServerCard";

const AUTOSAVE_DELAY_MS = 400;

const EMPTY_SERVER: SshServerConfig = {
  id: "",
  name: "",
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

const REVIEW_PROMPT_FIELD_ID = "ssh-review.systemPrompt";

/**
 * 设置弹窗的「SSH 服务器」页：服务器列表（自动保存）、命令审查 AI（引用制）、审计记录。
 * 外层弹窗提供滚动容器、header 与 AhaSettingsProvider，本页不渲染弹窗级元素。
 */
export function SshServersPage() {
  const { settings, updateSettings, saveError: settingsSaveError } = useAhaSettings();
  const [config, setConfig] = useState<SshToolsConfig>({ servers: [] });
  const [audit, setAudit] = useState<SshAuditLog>({ records: [] });
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<{ fieldId?: string; message: string } | null>(null);
  const [testRecords, setTestRecords] = useState<Record<string, ProviderTestRecord>>(() =>
    loadSshTestRecords(),
  );
  const [expandedAudit, setExpandedAudit] = useState<string | null>(null);
  const [pendingDeleteIndex, setPendingDeleteIndex] = useState<number | null>(null);
  // 导入本机 ~/.ssh/config：非空时展示选择对话框。
  const [importEntries, setImportEntries] = useState<SshServerConfig[] | null>(null);
  const [importing, setImporting] = useState(false);
  // 每台服务器默认折叠，仅在用户展开或新增时展开其详细配置。
  const [expandedServers, setExpandedServers] = useState<Set<number>>(new Set());

  // 自动保存是异步的，通过 ref 读取最新状态，避免闭包捕获过期值。
  const configRef = useRef(config);
  configRef.current = config;
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const fieldIdRef = useRef<string | undefined>(undefined);
  const savingRef = useRef(false);

  const loadConfig = useCallback(async () => {
    setLoading(true);
    setLoadError(null);
    try {
      const [loadedConfig, loadedAudit] = await Promise.all([
        invoke<SshToolsConfig>("ssh_tool_load_config"),
        invoke<SshAuditLog>("ssh_tool_load_audit"),
      ]);
      setConfig(loadedConfig);
      setAudit(loadedAudit);
    } catch (err) {
      setLoadError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadConfig();
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, [loadConfig]);

  const saveNow = useCallback(async () => {
    if (savingRef.current) return;
    savingRef.current = true;
    try {
      const savedConfig = await invoke<SshToolsConfig>("ssh_tool_save_config", {
        config: configRef.current,
      });
      setConfig(savedConfig);
      setSaveError(null);
      toast.success("已保存");
    } catch (err) {
      const message = String(err);
      setSaveError({ fieldId: fieldIdRef.current, message });
      toast.error(`保存失败：${message}`);
    } finally {
      savingRef.current = false;
    }
  }, []);

  const scheduleSave = useCallback(
    (fieldId?: string) => {
      fieldIdRef.current = fieldId ?? fieldIdRef.current;
      setSaveError(null);
      if (timerRef.current) clearTimeout(timerRef.current);
      timerRef.current = setTimeout(() => {
        timerRef.current = null;
        void saveNow();
      }, AUTOSAVE_DELAY_MS);
    },
    [saveNow],
  );

  function updateServer(
    index: number,
    updater: (server: SshServerConfig) => SshServerConfig,
    fieldId?: string,
  ) {
    setConfig((prev) => ({
      servers: prev.servers.map((server, i) => (i === index ? updater(server) : server)),
    }));
    scheduleSave(fieldId);
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
    scheduleSave();
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
    scheduleSave();
  }

  function handleTested(serverId: string, status: "ok" | "failed") {
    const record: ProviderTestRecord = { status, at: Date.now() };
    recordSshTest(serverId, record);
    setTestRecords((prev) => ({ ...prev, [serverId]: record }));
  }

  async function openImport() {
    setImporting(true);
    try {
      const entries = await invoke<SshServerConfig[]>("ssh_tool_import_ssh_config");
      if (entries.length === 0) {
        toast.success("~/.ssh/config 中没有可导入的主机条目");
      } else {
        setImportEntries(entries);
      }
    } catch (err) {
      toast.error(`读取本机 SSH 配置失败：${String(err)}`);
    } finally {
      setImporting(false);
    }
  }

  function confirmImport(selected: SshServerConfig[]) {
    if (selected.length === 0) {
      setImportEntries(null);
      return;
    }
    const startIndex = config.servers.length;
    setConfig((prev) => {
      // 与现有服务器及批次内部做 id 去重（导入条目之间已在后端去重）。
      const used = new Set(prev.servers.map((server) => server.id));
      const appended = selected.map((entry) => {
        const base = entry.id || nextServerId([...prev.servers]);
        const id = uniqueImportServerId(base, used);
        used.add(id);
        return { ...entry, id };
      });
      return { servers: [...prev.servers, ...appended] };
    });
    // 新导入的条目自动展开，方便补充密码等未导入的凭据。
    setExpandedServers((prev) => {
      const next = new Set(prev);
      for (let offset = 0; offset < selected.length; offset += 1) {
        next.add(startIndex + offset);
      }
      return next;
    });
    setImportEntries(null);
    // 后端校验是全有或全无：缺凭据的条目保存必失败，还会连累整批配置。
    // 存在不完整条目时不立即触发保存（等用户补全凭据后的编辑再触发），
    // 并明确提示需要补全，避免用户只看到笼统的保存失败。
    const incompleteCount = selected.filter(
      (entry) =>
        (entry.authMethod === "password" && entry.password.trim() === "") ||
        (entry.authMethod === "key" && entry.privateKeyPath.trim() === ""),
    ).length;
    if (incompleteCount > 0) {
      toast.success(
        `已导入 ${selected.length} 台服务器，其中 ${incompleteCount} 台需补全密码或密钥路径后才能保存`,
      );
    } else {
      scheduleSave();
      toast.success(`已导入 ${selected.length} 台服务器`);
    }
  }

  const pendingDeleteServer =
    pendingDeleteIndex !== null ? config.servers[pendingDeleteIndex] : undefined;
  const reviewModel = settings?.review?.modelConfig;
  const reviewPrompt = settings?.review?.systemPrompt ?? DEFAULT_REVIEW_PROMPT;

  return (
    <div className="ai-set-page">
      <Section
        id="ssh-review"
        title="命令审查 AI"
        description="SSH 与本地命令执行前会交由审查模型，结合意图/任务/目标环境/命令做安全评估；审查异常或判定不通过将阻断执行。该配置全局共享（项目与聊天通用）。"
      >
        <div className="flex flex-col gap-3">
          <div className="ai-set-field">
            <FieldLabel
              label="审查模型"
              tip="命令安全审查使用的模型，采用引用制：此处只读展示当前绑定，不可直接编辑。"
            />
            <div className="ai-set-model-ref">
              {reviewModel && reviewModel.model.trim() ? (
                <span className="ai-set-model-ref-text">
                  {reviewModel.url.trim() || "未设置 URL"}
                  <span className="ai-set-model-ref-sep">·</span>
                  {reviewModel.model}
                </span>
              ) : (
                <span className="ai-set-model-ref-text is-empty">尚未绑定审查模型</span>
              )}
            </div>
            <p className="ai-settings-hint">在「模型用途」页可更换审查模型。</p>
          </div>

          <div className="ai-set-field">
            <FieldLabel
              label="系统提示词"
              tip="约束审查 AI 的判定原则与输出格式，修改后失焦自动保存。"
            />
            <CommitTextarea
              value={reviewPrompt}
              onCommit={(next) =>
                updateSettings(
                  (prev) => ({
                    ...prev,
                    review: { ...prev.review, systemPrompt: next },
                  }),
                  REVIEW_PROMPT_FIELD_ID,
                )
              }
            />
            {settingsSaveError?.fieldId === REVIEW_PROMPT_FIELD_ID && (
              <p className="ai-set-field-error">{settingsSaveError.message}</p>
            )}
            <div>
              <button
                type="button"
                className="ai-set-ghost-button"
                onClick={() =>
                  updateSettings(
                    (prev) => ({
                      ...prev,
                      review: { ...prev.review, systemPrompt: DEFAULT_REVIEW_PROMPT },
                    }),
                    REVIEW_PROMPT_FIELD_ID,
                  )
                }
              >
                <RotateCcw size={16} strokeWidth={1.5} />
                恢复默认提示词
              </button>
            </div>
          </div>
        </div>
      </Section>

      <Section
        id="ssh-servers"
        title="SSH 服务器"
        description="SSH 服务器为应用全局配置，所有项目与聊天共享同一份服务器列表；凭据、审计与主机密钥固定存储在应用数据库（~/.jkcodingagent/jkbot.sqlite3）中，不在项目仓库内，智能体文件工具无法读取。字段失焦或开关切换后自动保存。"
      >
        <div className="flex flex-col gap-3">
          <div className="flex flex-wrap items-center gap-2">
            <div className="flex-1" />
            <button
              type="button"
              className="ai-set-ghost-button"
              onClick={loadConfig}
              disabled={loading}
            >
              <RefreshCw size={16} strokeWidth={1.5} />
              刷新
            </button>
            <button
              type="button"
              className="ai-set-ghost-button"
              onClick={openImport}
              disabled={importing}
              title="从 ~/.ssh/config 解析 Host 条目并导入"
            >
              <Download size={16} strokeWidth={1.5} />
              {importing ? "读取中..." : "导入本机配置"}
            </button>
            <button type="button" className="ai-set-ghost-button" onClick={addServer}>
              <Plus size={16} strokeWidth={1.5} />
              添加服务器
            </button>
          </div>

          {loadError && <p className="ai-set-field-error">{loadError}</p>}

          {loading ? (
            <div className="ai-settings-empty">加载中...</div>
          ) : config.servers.length === 0 ? (
            <EmptyState
              icon={Server}
              title="还没有 SSH 服务器"
              actionLabel="添加服务器"
              onAction={addServer}
            />
          ) : (
            <div className="flex flex-col gap-2">
              {config.servers.map((server, index) => (
                <SshServerCard
                  key={`${server.id}-${index}`}
                  server={server}
                  expanded={expandedServers.has(index)}
                  testRecord={server.id ? testRecords[server.id] : undefined}
                  errorFieldId={saveError?.fieldId}
                  errorMessage={saveError?.message}
                  onToggleExpand={() => toggleServer(index)}
                  onUpdate={(updater, fieldId) => updateServer(index, updater, fieldId)}
                  onRemove={() => setPendingDeleteIndex(index)}
                  onTested={(status) => {
                    if (server.id.trim()) handleTested(server.id, status);
                  }}
                />
              ))}
            </div>
          )}
        </div>
      </Section>

      <Section
        id="ssh-audit"
        title="SSH 审计记录"
        description="保留最近的 100 条命令记录，包含审查结论、会话、命令和执行结果。"
      >
        <div className="flex flex-col gap-2">
          <div className="flex justify-end">
            <span className="ai-ssh-count-pill">{audit.records.length}/100</span>
          </div>
          <SshAuditRecordList
            records={audit.records}
            expandedAudit={expandedAudit}
            onExpandedAuditChange={setExpandedAudit}
          />
        </div>
      </Section>

      {importEntries && (
        <SshImportDialog
          entries={importEntries}
          existing={config.servers}
          onConfirm={confirmImport}
          onCancel={() => setImportEntries(null)}
        />
      )}

      <ConfirmDialog
        open={pendingDeleteIndex !== null}
        title="删除 SSH 服务器"
        description={
          pendingDeleteServer
            ? `删除后智能体将无法通过 SSH 连接到 ${pendingDeleteServer.username.trim() || "（未设置用户名）"}@${pendingDeleteServer.host.trim() || "（未设置主机）"}。`
            : ""
        }
        confirmLabel="删除"
        onConfirm={() => {
          if (pendingDeleteIndex !== null) removeServer(pendingDeleteIndex);
          setPendingDeleteIndex(null);
        }}
        onCancel={() => setPendingDeleteIndex(null)}
      />
    </div>
  );
}

/** 失焦提交多行文本框：本地草稿编辑，blur 时值有变化才回调。 */
function CommitTextarea({
  value,
  onCommit,
}: {
  value: string;
  onCommit: (next: string) => void;
}) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  return (
    <textarea
      className="ai-settings-textarea ai-set-prompt-textarea"
      value={draft}
      spellCheck={false}
      onChange={(event) => setDraft(event.target.value)}
      onBlur={() => {
        if (draft !== value) onCommit(draft);
      }}
    />
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

// 与后端 ssh_tool 的 ID_MAX_LEN 保持一致：满长 id 冲突时追加 `-N` 后缀须先
// 为后缀预留空间，否则超长 id 会在保存时被后端校验整批拒绝。
const SSH_SERVER_ID_MAX_LEN = 64;

function uniqueImportServerId(base: string, used: Set<string>): string {
  if (!used.has(base)) return base;
  let suffix = 2;
  for (;;) {
    const suffixText = `-${suffix}`;
    const truncated = base.slice(0, SSH_SERVER_ID_MAX_LEN - suffixText.length);
    const candidate = `${truncated}${suffixText}`;
    if (!used.has(candidate)) return candidate;
    suffix += 1;
  }
}

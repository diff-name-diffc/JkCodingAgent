import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Plus, RefreshCw, RotateCcw, Server } from "lucide-react";
import type { AgentContext, SshAuditLog, SshServerConfig, SshToolsConfig } from "../../../types";
import { cn } from "../../../lib/cn";
import { SshAuditRecordList } from "../../app-settings/aha/SshAuditRecordList";
import { ConfirmDialog } from "../ConfirmDialog";
import { EmptyState } from "../EmptyState";
import { FieldLabel } from "../FieldLabel";
import { Section } from "../Section";
import { toast } from "../toast";
import { Tooltip, TooltipContent, TooltipTrigger } from "../../ui/tooltip";
import { useAhaSettings } from "../use-aha-settings";
import {
  loadSshTestRecords,
  recordSshTest,
  type ProviderTestRecord,
} from "../providers/provider-prefs";
import { SshServerCard } from "./SshServerCard";

const AUTOSAVE_DELAY_MS = 400;

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

const REVIEW_PROMPT_FIELD_ID = "ssh-review.systemPrompt";

/**
 * 设置弹窗的「SSH 服务器」页：服务器列表（自动保存）、命令审查 AI（引用制）、审计记录。
 * 外层弹窗提供滚动容器、header 与 AhaSettingsProvider，本页不渲染弹窗级元素。
 */
export function SshServersPage({ projectPath }: { projectPath?: string }) {
  const { settings, updateSettings, saveError: settingsSaveError } = useAhaSettings();
  const [context, setContext] = useState<AgentContext>(projectPath ? "project" : "chat");
  const [workspacePath, setWorkspacePath] = useState("");
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
  // 每台服务器默认折叠，仅在用户展开或新增时展开其详细配置。
  const [expandedServers, setExpandedServers] = useState<Set<number>>(new Set());

  // 自动保存是异步的，通过 ref 读取最新状态，避免闭包捕获过期值。
  const configRef = useRef(config);
  configRef.current = config;
  const workspaceRef = useRef(workspacePath);
  workspaceRef.current = workspacePath;
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const fieldIdRef = useRef<string | undefined>(undefined);
  const savingRef = useRef(false);

  const loadConfig = useCallback(async () => {
    if (context === "project" && !projectPath) {
      setLoadError("当前没有项目路径，请切换到聊天 SSH 配置。");
      return;
    }
    setLoading(true);
    setLoadError(null);
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
    } catch (err) {
      setLoadError(String(err));
    } finally {
      setLoading(false);
    }
  }, [context, projectPath]);

  useEffect(() => {
    loadConfig();
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, [loadConfig]);

  const saveNow = useCallback(async () => {
    const workspace = workspaceRef.current;
    if (!workspace || savingRef.current) return;
    savingRef.current = true;
    try {
      const savedConfig = await invoke<SshToolsConfig>("ssh_tool_save_config", {
        projectPath: workspace,
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
        description="项目和聊天分别使用自己的本地 SSH 环境；配置与审计文件位于工作区的 .jkcodingagent/local_env/ssh。字段失焦或开关切换后自动保存。"
      >
        <div className="flex flex-col gap-3">
          <div className="flex flex-wrap items-center gap-2">
            <FieldLabel
              label="配置范围"
              tip={`「项目」配置仅当前项目可用，「聊天」配置用于不绑定项目的独立聊天工作区。${
                projectPath ? "" : "当前未绑定项目，「项目」不可选。"
              }`}
            />
            {projectPath ? (
              <button
                type="button"
                className={cn("ai-aha-category-chip", context === "project" && "is-active")}
                onClick={() => setContext("project")}
              >
                项目
              </button>
            ) : (
              // disabled 按钮不触发指针事件，用 span 承载 Tooltip 才能悬停显示原因。
              <Tooltip>
                <TooltipTrigger asChild>
                  <span className="inline-flex" tabIndex={0}>
                    <button type="button" className="ai-aha-category-chip" disabled>
                      项目
                    </button>
                  </span>
                </TooltipTrigger>
                <TooltipContent side="top" className="max-w-64">
                  当前未绑定项目：请在项目工作区内打开设置，再编辑「项目」范围的 SSH 配置。
                </TooltipContent>
              </Tooltip>
            )}
            <button
              type="button"
              className={cn("ai-aha-category-chip", context === "chat" && "is-active")}
              onClick={() => setContext("chat")}
            >
              聊天
            </button>
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
            <button type="button" className="ai-set-ghost-button" onClick={addServer}>
              <Plus size={16} strokeWidth={1.5} />
              添加服务器
            </button>
          </div>

          {workspacePath && (
            <p className="ai-settings-hint">
              当前工作区：<span className="font-mono">{workspacePath}</span>
            </p>
          )}

          {!projectPath && (
            <p className="ai-settings-hint">
              设置从未绑定项目的入口打开，仅可编辑「聊天」范围；进入项目后从项目内打开设置，即可编辑「项目」范围。
            </p>
          )}

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

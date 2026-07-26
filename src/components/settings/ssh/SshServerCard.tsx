import { useEffect, useState } from "react";
import type { InputHTMLAttributes, ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { ChevronDown, FolderOpen, Trash2 } from "lucide-react";
import type { SshServerConfig } from "../../../types";
import { cn } from "../../../lib/cn";
import { Tooltip, TooltipContent, TooltipTrigger } from "../../ui/tooltip";
import { ApiKeyInput } from "../ApiKeyInput";
import { FieldLabel } from "../FieldLabel";
import { TestButton } from "../TestButton";
import type { ProviderTestRecord } from "../providers/provider-prefs";

/**
 * 单台 SSH 服务器的折叠卡片：头部为启用开关 + 状态点 + 标题 + 测试/删除，
 * 展开后是连接与认证表单。文本字段失焦（值有变化时）才通过 onUpdate 提交，
 * 由父组件统一 debounce 自动保存；开关类字段变更即提交。
 */
export function SshServerCard({
  server,
  expanded,
  testRecord,
  errorFieldId,
  errorMessage,
  onToggleExpand,
  onUpdate,
  onRemove,
  onTested,
}: {
  server: SshServerConfig;
  expanded: boolean;
  testRecord?: ProviderTestRecord;
  /** 最近一次自动保存失败对应的字段 id（ssh.server.{id}.{field}）。 */
  errorFieldId?: string;
  errorMessage?: string;
  onToggleExpand: () => void;
  onUpdate: (updater: (server: SshServerConfig) => SshServerConfig, fieldId?: string) => void;
  onRemove: () => void;
  onTested: (status: "ok" | "failed") => void;
}) {
  const fid = (field: string) => `ssh.server.${server.id}.${field}`;
  const fieldError = (field: string) =>
    errorFieldId === fid(field) ? errorMessage : undefined;

  async function pickKeyFile() {
    const selected = await openDialog({
      directory: false,
      multiple: false,
      title: "选择 SSH 私钥文件（如 ~/.ssh/id_rsa、id_ed25519）",
    });
    if (typeof selected === "string" && selected.length > 0) {
      onUpdate((draft) => ({ ...draft, privateKeyPath: selected }), fid("privateKeyPath"));
    }
  }

  return (
    <div className="ai-set-server-card">
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-2">
          <input
            type="checkbox"
            checked={server.enabled}
            title={server.enabled ? "已启用" : "已停用"}
            onChange={(event) =>
              onUpdate((draft) => ({ ...draft, enabled: event.target.checked }), fid("enabled"))
            }
          />
          <StatusDot record={testRecord} />
          <button type="button" className="ai-set-server-title-btn" onClick={onToggleExpand}>
            <ChevronDown
              size={14}
              strokeWidth={1.5}
              style={{
                transform: expanded ? "rotate(0deg)" : "rotate(-90deg)",
                transition: "transform var(--motion-fast) var(--motion-ease)",
              }}
            />
            <span className="ai-set-server-title">{server.id || "未命名服务器"}</span>
            <span className="ai-set-server-summary">{serverSummary(server)}</span>
          </button>
        </div>
        <div className="flex flex-shrink-0 items-center gap-2">
          <TestButton
            disabled={!server.id.trim()}
            onTest={() => invoke<string>("ssh_tool_test_server_config", { server })}
            onResult={(result) => {
              if (!result) return;
              onTested(result.status === "success" ? "ok" : "failed");
            }}
          />
          <button type="button" className="ai-set-ghost-button is-danger" onClick={onRemove}>
            <Trash2 size={16} strokeWidth={1.5} />
            删除
          </button>
        </div>
      </div>

      {expanded && (
        <div className="mt-3 flex flex-col gap-3">
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={server.reviewEnabled}
              onChange={(event) =>
                onUpdate(
                  (draft) => ({ ...draft, reviewEnabled: event.target.checked }),
                  fid("reviewEnabled"),
                )
              }
            />
            <FieldLabel
              label="执行前审查"
              tip="命令安全审查：开启后，智能体在这台服务器上执行的每条命令会先交由审查 AI 评估安全性，判定不通过或审查异常时阻断执行。"
            />
            {fieldError("reviewEnabled") && (
              <span className="ai-set-field-error">{fieldError("reviewEnabled")}</span>
            )}
          </label>

          <div className="ai-set-form-grid">
            <Field label="Server ID" error={fieldError("id")}>
              <CommitInput
                value={server.id}
                placeholder="prod-web-1"
                onCommit={(next) =>
                  onUpdate((draft) => ({ ...draft, id: next.trim().toLowerCase() }), fid("id"))
                }
              />
            </Field>
            <Field label="描述" error={fieldError("description")}>
              <CommitInput
                value={server.description}
                placeholder="生产 Web 节点"
                onCommit={(next) =>
                  onUpdate((draft) => ({ ...draft, description: next }), fid("description"))
                }
              />
            </Field>
            <Field label="Host" error={fieldError("host")}>
              <CommitInput
                value={server.host}
                placeholder="10.0.1.12"
                onCommit={(next) => onUpdate((draft) => ({ ...draft, host: next }), fid("host"))}
              />
            </Field>
            <Field label="Port" error={fieldError("port")}>
              <CommitNumberInput
                value={server.port}
                min={1}
                max={65535}
                fallback={22}
                onCommit={(next) => onUpdate((draft) => ({ ...draft, port: next }), fid("port"))}
              />
            </Field>
            <Field label="Username" error={fieldError("username")}>
              <CommitInput
                value={server.username}
                onCommit={(next) =>
                  onUpdate((draft) => ({ ...draft, username: next }), fid("username"))
                }
              />
            </Field>
            <Field
              label="超时时间（秒）"
              tip="单条命令的最长执行时间，超时后自动中止。"
              error={fieldError("defaultTimeoutSecs")}
            >
              <CommitNumberInput
                value={server.defaultTimeoutSecs}
                min={1}
                max={300}
                fallback={30}
                onCommit={(next) =>
                  onUpdate(
                    (draft) => ({ ...draft, defaultTimeoutSecs: next }),
                    fid("defaultTimeoutSecs"),
                  )
                }
              />
            </Field>
            <Field
              label="最大输出字节"
              tip="命令输出超过该字节数后会被截断，避免超大输出占满上下文。"
              error={fieldError("maxOutputBytes")}
            >
              <CommitNumberInput
                value={server.maxOutputBytes}
                min={1024}
                max={1048576}
                fallback={65536}
                onCommit={(next) =>
                  onUpdate((draft) => ({ ...draft, maxOutputBytes: next }), fid("maxOutputBytes"))
                }
              />
            </Field>
          </div>

          <Field label="认证方式">
            <div className="flex gap-2">
              <button
                type="button"
                className={cn(
                  "ai-aha-category-chip",
                  server.authMethod === "password" && "is-active",
                )}
                onClick={() =>
                  onUpdate((draft) => ({ ...draft, authMethod: "password" }), fid("authMethod"))
                }
              >
                密码
              </button>
              <button
                type="button"
                className={cn(
                  "ai-aha-category-chip",
                  server.authMethod === "key" && "is-active",
                )}
                onClick={() =>
                  onUpdate((draft) => ({ ...draft, authMethod: "key" }), fid("authMethod"))
                }
              >
                私钥文件
              </button>
            </div>
          </Field>

          {server.authMethod === "password" ? (
            <Field label="密码" error={fieldError("password")}>
              <CommitSecret
                value={server.password}
                placeholder="SSH 登录密码"
                onCommit={(next) =>
                  onUpdate((draft) => ({ ...draft, password: next }), fid("password"))
                }
              />
            </Field>
          ) : (
            <>
              <Field label="私钥文件路径" error={fieldError("privateKeyPath")}>
                <div className="flex items-center gap-2">
                  <CommitInput
                    value={server.privateKeyPath}
                    placeholder="~/.ssh/id_rsa"
                    onCommit={(next) =>
                      onUpdate(
                        (draft) => ({ ...draft, privateKeyPath: next }),
                        fid("privateKeyPath"),
                      )
                    }
                  />
                  <button
                    type="button"
                    className="ai-set-ghost-button flex-shrink-0"
                    onClick={pickKeyFile}
                  >
                    <FolderOpen size={16} strokeWidth={1.5} />
                    选择
                  </button>
                </div>
              </Field>
              <Field
                label="私钥口令（可选）"
                tip="私钥文件本身有密码保护时填写；未加密的私钥留空即可。"
                error={fieldError("privateKeyPassphrase")}
              >
                <CommitSecret
                  value={server.privateKeyPassphrase}
                  placeholder="加密私钥的口令"
                  onCommit={(next) =>
                    onUpdate(
                      (draft) => ({ ...draft, privateKeyPassphrase: next }),
                      fid("privateKeyPassphrase"),
                    )
                  }
                />
              </Field>
            </>
          )}

          <Field
            label="标签"
            tip="用英文逗号分隔，便于智能体按用途筛选服务器，例如：prod, web。"
            error={fieldError("tags")}
          >
            <CommitInput
              value={server.tags.join(", ")}
              placeholder="prod, web"
              onCommit={(next) =>
                onUpdate(
                  (draft) => ({
                    ...draft,
                    tags: next
                      .split(",")
                      .map((tag) => tag.trim())
                      .filter(Boolean),
                  }),
                  fid("tags"),
                )
              }
            />
          </Field>
        </div>
      )}
    </div>
  );
}

/** 最近测试状态点：绿=通过 / 红=失败 / 灰=未测试，hover 显示最后测试时间。 */
function StatusDot({ record }: { record?: ProviderTestRecord }) {
  const status = !record ? "untested" : record.status === "ok" ? "ok" : "failed";
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span className={cn("ai-set-status-dot", `is-${status}`)} />
      </TooltipTrigger>
      <TooltipContent side="top">
        {record
          ? `最后测试：${formatTimestamp(record.at)}（${record.status === "ok" ? "连接成功" : "连接失败"}）`
          : "尚未测试"}
      </TooltipContent>
    </Tooltip>
  );
}

function Field({
  label,
  tip,
  error,
  children,
}: {
  label: string;
  tip?: string;
  error?: string;
  children: ReactNode;
}) {
  return (
    <div className="ai-set-field">
      <FieldLabel label={label} tip={tip} />
      {children}
      {error && <p className="ai-set-field-error">{error}</p>}
    </div>
  );
}

/** 失焦提交文本框：本地草稿编辑，blur 时值有变化才回调。 */
function CommitInput({
  value,
  onCommit,
  ...rest
}: {
  value: string;
  onCommit: (next: string) => void;
} & Omit<InputHTMLAttributes<HTMLInputElement>, "value" | "onChange" | "onBlur" | "type">) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  return (
    <input
      className="ai-settings-input"
      value={draft}
      onChange={(event) => setDraft(event.target.value)}
      onBlur={() => {
        if (draft !== value) onCommit(draft);
      }}
      {...rest}
    />
  );
}

/** 失焦提交数字框：解析失败时回退到 fallback。 */
function CommitNumberInput({
  value,
  fallback,
  onCommit,
  ...rest
}: {
  value: number;
  fallback: number;
  onCommit: (next: number) => void;
} & Omit<InputHTMLAttributes<HTMLInputElement>, "value" | "onChange" | "onBlur" | "type">) {
  const [draft, setDraft] = useState(String(value));
  useEffect(() => setDraft(String(value)), [value]);
  return (
    <input
      className="ai-settings-input"
      type="number"
      value={draft}
      onChange={(event) => setDraft(event.target.value)}
      onBlur={() => {
        const parsed = Number(draft) || fallback;
        if (parsed !== value) onCommit(parsed);
        else setDraft(String(value));
      }}
      {...rest}
    />
  );
}

/** 失焦提交的密码/口令输入（带明文切换）。 */
function CommitSecret({
  value,
  placeholder,
  onCommit,
}: {
  value: string;
  placeholder?: string;
  onCommit: (next: string) => void;
}) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  return (
    <ApiKeyInput
      value={draft}
      placeholder={placeholder}
      onChange={setDraft}
      onBlur={() => {
        if (draft !== value) onCommit(draft);
      }}
    />
  );
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

function formatTimestamp(at: number): string {
  const date = new Date(at);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

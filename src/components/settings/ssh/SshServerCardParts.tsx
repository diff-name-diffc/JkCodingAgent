import { useEffect, useState } from "react";
import type { InputHTMLAttributes, ReactNode } from "react";
import { Tooltip, TooltipContent, TooltipTrigger } from "../../ui/tooltip";
import { ApiKeyInput } from "../ApiKeyInput";
import { FieldLabel } from "../FieldLabel";
import { StatusBadge } from "../StatusBadge";
import type { ProviderTestRecord } from "../providers/provider-prefs";
import type { SshServerConfig } from "../../../types";

/**
 * SshServerCard 的私有展示组件集合（拆分自 SshServerCard.tsx，控制单文件
 * 规模）。失焦提交语义：本地草稿编辑，blur 时值有变化才回调 onCommit，
 * 由卡片/页面统一 debounce 自动保存。
 */

/**
 * 最近测试状态（UI-22c）：双编码徽标（色点 + 文字「可用/失败/未测试」）取代旧
 * 「纯色点 + 仅 hover tooltip 文字」——状态无需悬停即可读，收敛 tokens.md §5
 * 结论 4「不得只靠彩点」。复用设置域既有 StatusBadge，与模型服务测试状态一致；
 * tooltip 保留最后测试时间作为补充信息。
 */
export function TestStatusBadge({ record }: { record?: ProviderTestRecord }) {
  const status = !record ? "untested" : record.status === "ok" ? "ok" : "failed";
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span>
          <StatusBadge status={status} />
        </span>
      </TooltipTrigger>
      <TooltipContent side="top">
        {record
          ? `最后测试：${formatTimestamp(record.at)}（${record.status === "ok" ? "连接成功" : "连接失败"}）`
          : "尚未测试"}
      </TooltipContent>
    </Tooltip>
  );
}

export function Field({
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
export function CommitInput({
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
export function CommitNumberInput({
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
export function CommitSecret({
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
export function serverSummary(server: SshServerConfig): string {
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

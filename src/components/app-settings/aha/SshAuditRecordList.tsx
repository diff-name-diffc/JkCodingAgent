import type React from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import type { SshAuditRecord } from "../../../types";

export function SshAuditRecordList({
  records,
  expandedAudit,
  onExpandedAuditChange,
}: {
  records: SshAuditRecord[];
  expandedAudit: string | null;
  onExpandedAuditChange: (key: string | null) => void;
}) {
  if (records.length === 0) {
    return <div className="ai-settings-empty">暂无审计记录。</div>;
  }

  return (
    <div className="ai-ssh-audit-list">
      {records.map((record, index) => {
        const auditKey = `${record.createdAt}-${index}`;
        const expanded = expandedAudit === auditKey;
        return (
          <SshAuditRecordItem
            key={auditKey}
            record={record}
            expanded={expanded}
            onToggle={() => onExpandedAuditChange(expanded ? null : auditKey)}
          />
        );
      })}
    </div>
  );
}

function SshAuditRecordItem({
  record,
  expanded,
  onToggle,
}: {
  record: SshAuditRecord;
  expanded: boolean;
  onToggle: () => void;
}) {
  return (
    <div className="ai-ssh-audit-record">
      <button type="button" className="ai-ssh-audit-header" onClick={onToggle}>
        <div className="ai-ssh-audit-title-wrap">
          {expanded ? (
            <ChevronDown size={13} />
          ) : (
            <ChevronRight size={13} />
          )}
          <span className="ai-ssh-audit-server">{record.serverId}</span>
          <span className="ai-ssh-audit-session">{record.sessionId}</span>
          <ReviewBadge record={record} />
          <ExecutionBadge record={record} />
        </div>
        <span className="ai-ssh-audit-date">{record.createdAt}</span>
      </button>

      <pre className="ai-ssh-audit-pre">{record.command}</pre>
      <div className="ai-ssh-audit-summary">{executionSummary(record)}</div>

      {expanded && (
        <div className="ai-ssh-audit-details">
          <AuditInfoBlock title="命令审查 AI" tone={reviewTone(record)}>
            {reviewText(record)}
          </AuditInfoBlock>
          <AuditInfoBlock title="执行结果" tone={executionTone(record)}>
            {executionText(record)}
          </AuditInfoBlock>
          <OutputBlock title="stdout" value={record.stdout} emptyText="无标准输出" />
          <OutputBlock title="stderr" value={record.stderr} emptyText="无错误输出" />
          {record.error ? (
            <AuditInfoBlock title="错误" tone="danger">
              {record.error}
            </AuditInfoBlock>
          ) : null}
        </div>
      )}
    </div>
  );
}

function ReviewBadge({ record }: { record: SshAuditRecord }) {
  if (!record.review) {
    return <span className="ai-ssh-badge is-muted">未审查</span>;
  }
  return (
    <span className={record.review.allowed ? "ai-ssh-badge is-success" : "ai-ssh-badge is-danger"}>
      {record.review.allowed ? "审查通过" : "审查拦截"}
    </span>
  );
}

function ExecutionBadge({ record }: { record: SshAuditRecord }) {
  if (record.review && !record.review.allowed) {
    return <span className="ai-ssh-badge is-muted">未执行</span>;
  }
  if (record.interactiveBlocked) {
    return <span className="ai-ssh-badge is-warning">交互阻塞</span>;
  }
  if (record.error) {
    return <span className="ai-ssh-badge is-danger">执行失败</span>;
  }
  if (record.exitCode != null && record.exitCode !== 0) {
    return <span className="ai-ssh-badge is-warning">退出异常</span>;
  }
  return <span className="ai-ssh-badge is-success">已执行</span>;
}

function AuditInfoBlock({
  title,
  tone,
  children,
}: {
  title: string;
  tone: "success" | "danger" | "warning" | "muted";
  children: React.ReactNode;
}) {
  return (
    <div className={`ai-ssh-info-block is-${tone}`}>
      <div className="ai-ssh-info-title">{title}</div>
      <div className="ai-ssh-info-body">{children}</div>
    </div>
  );
}

function OutputBlock({
  title,
  value,
  emptyText,
}: {
  title: string;
  value: string;
  emptyText: string;
}) {
  const trimmed = value.trim();
  return (
    <div>
      <div className="ai-ssh-output-title">{title}</div>
      <pre className="ai-ssh-audit-pre ai-ssh-output-pre">
        {trimmed.length > 0 ? value : emptyText}
      </pre>
    </div>
  );
}

function reviewText(record: SshAuditRecord): string {
  if (!record.review) {
    return "未启用命令审查 AI，或该服务器关闭了执行前审查。";
  }
  if (record.review.allowed) {
    return record.review.reason || "审查通过，允许执行。";
  }
  return record.review.reason || "审查拒绝，命令未执行。";
}

function executionText(record: SshAuditRecord): string {
  if (record.review && !record.review.allowed) {
    return "未执行：命令被审查 AI 拦截。";
  }
  if (record.interactiveBlocked) {
    return "已中止：命令需要交互式输入，SSH 工具按非交互策略阻断。";
  }
  if (record.error) {
    return `执行失败：${record.error}`;
  }
  if (record.exitCode != null && record.exitCode !== 0) {
    return `执行完成但退出码非 0：exit=${record.exitCode}，duration=${record.durationMs ?? "-"}ms。`;
  }
  return `执行完成：exit=${record.exitCode ?? "unknown"}，duration=${record.durationMs ?? "-"}ms。`;
}

function executionSummary(record: SshAuditRecord): string {
  const exit =
    record.review && !record.review.allowed
      ? "审查拦截(未执行)"
      : record.interactiveBlocked
        ? "交互阻塞(已中止)"
        : (record.exitCode ?? "error");
  const parts = [`exit=${exit}`, `duration=${record.durationMs ?? "-"}ms`];
  if (record.truncated) {
    parts.push("truncated");
  }
  if (record.error) {
    parts.push(record.error);
  }
  return parts.join(" · ");
}

function reviewTone(record: SshAuditRecord): "success" | "danger" | "muted" {
  if (!record.review) {
    return "muted";
  }
  return record.review.allowed ? "success" : "danger";
}

function executionTone(record: SshAuditRecord): "success" | "danger" | "warning" | "muted" {
  if (record.review && !record.review.allowed) {
    return "muted";
  }
  if (record.interactiveBlocked) {
    return "warning";
  }
  if (record.error) {
    return "danger";
  }
  if (record.exitCode != null && record.exitCode !== 0) {
    return "warning";
  }
  return "success";
}

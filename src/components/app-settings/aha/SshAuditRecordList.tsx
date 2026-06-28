import type React from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import type { SshAuditRecord } from "../../../types";
import s from "../../../styles";

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
    return <div style={s.ahaHint}>暂无审计记录。</div>;
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
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
    <div style={s.ahaProvider}>
      <button type="button" style={auditHeaderButtonStyle} onClick={onToggle}>
        <div style={s.ahaProviderTitleWrap}>
          {expanded ? (
            <ChevronDown size={13} style={{ flexShrink: 0 }} />
          ) : (
            <ChevronRight size={13} style={{ flexShrink: 0 }} />
          )}
          <span style={s.ahaProviderTitle}>{record.serverId}</span>
          <span style={s.ahaProviderSummary}>{record.sessionId}</span>
          <ReviewBadge record={record} />
          <ExecutionBadge record={record} />
        </div>
        <span style={s.ahaHint}>{record.createdAt}</span>
      </button>

      <pre style={auditPreStyle}>{record.command}</pre>
      <div style={s.ahaHint}>{executionSummary(record)}</div>

      {expanded && (
        <div style={auditDetailsStyle}>
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
    return <span style={mutedBadgeStyle}>未审查</span>;
  }
  return (
    <span style={record.review.allowed ? reviewPassBadgeStyle : reviewBlockBadgeStyle}>
      {record.review.allowed ? "审查通过" : "审查拦截"}
    </span>
  );
}

function ExecutionBadge({ record }: { record: SshAuditRecord }) {
  if (record.review && !record.review.allowed) {
    return <span style={mutedBadgeStyle}>未执行</span>;
  }
  if (record.interactiveBlocked) {
    return <span style={warningBadgeStyle}>交互阻塞</span>;
  }
  if (record.error) {
    return <span style={reviewBlockBadgeStyle}>执行失败</span>;
  }
  if (record.exitCode != null && record.exitCode !== 0) {
    return <span style={warningBadgeStyle}>退出异常</span>;
  }
  return <span style={reviewPassBadgeStyle}>已执行</span>;
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
    <div style={{ ...infoBlockStyle, borderColor: toneColor(tone) }}>
      <div style={{ ...infoTitleStyle, color: toneColor(tone) }}>{title}</div>
      <div style={infoBodyStyle}>{children}</div>
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
      <div style={outputTitleStyle}>{title}</div>
      <pre style={{ ...auditPreStyle, ...outputPreStyle }}>
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

function toneColor(tone: "success" | "danger" | "warning" | "muted"): string {
  if (tone === "success") {
    return "var(--success)";
  }
  if (tone === "danger") {
    return "var(--danger)";
  }
  if (tone === "warning") {
    return "var(--warning)";
  }
  return "var(--border-dim)";
}

const auditHeaderButtonStyle: React.CSSProperties = {
  ...s.ahaProviderHeader,
  width: "100%",
  border: 0,
  padding: 0,
  background: "transparent",
  color: "inherit",
  cursor: "pointer",
  userSelect: "none",
  textAlign: "left",
};

const auditDetailsStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 8,
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

const badgeBaseStyle: React.CSSProperties = {
  fontSize: 10.5,
  fontWeight: 600,
  borderRadius: 6,
  padding: "0 6px",
  lineHeight: 1.5,
};

const reviewPassBadgeStyle: React.CSSProperties = {
  ...badgeBaseStyle,
  color: "var(--success)",
  border: "1px solid var(--success)",
};

const reviewBlockBadgeStyle: React.CSSProperties = {
  ...badgeBaseStyle,
  color: "var(--danger)",
  border: "1px solid var(--danger)",
};

const warningBadgeStyle: React.CSSProperties = {
  ...badgeBaseStyle,
  color: "var(--warning)",
  border: "1px solid var(--warning)",
};

const mutedBadgeStyle: React.CSSProperties = {
  ...badgeBaseStyle,
  color: "var(--text-secondary)",
  border: "1px solid var(--border-dim)",
};

const infoBlockStyle: React.CSSProperties = {
  margin: 0,
  padding: "8px 10px",
  borderRadius: 8,
  border: "1px solid var(--border-dim)",
  background: "var(--bg-subtle)",
  color: "var(--text-primary)",
  fontSize: 11.5,
  lineHeight: 1.5,
  whiteSpace: "pre-wrap",
  wordBreak: "break-word",
};

const infoTitleStyle: React.CSSProperties = {
  fontWeight: 700,
  marginBottom: 4,
};

const infoBodyStyle: React.CSSProperties = {
  color: "var(--text-primary)",
};

const outputTitleStyle: React.CSSProperties = {
  marginBottom: 4,
  color: "var(--text-secondary)",
  fontSize: 11,
  fontWeight: 700,
};

const outputPreStyle: React.CSSProperties = {
  maxHeight: 220,
  overflow: "auto",
  color: "var(--text-secondary)",
  fontSize: 11,
};

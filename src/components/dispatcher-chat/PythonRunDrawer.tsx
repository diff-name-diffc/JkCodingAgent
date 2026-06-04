import { memo, useMemo } from "react";
import type React from "react";
import { RotateCcw, Square, Trash2, X } from "lucide-react";
import type { PythonCodeRunRecord, PythonCodeRunTarget, PythonRunToolEvent } from "../../types";
import { MarkdownRenderer } from "../markdown/MarkdownRenderer";

interface PythonRunDrawerProps {
  target: PythonCodeRunTarget | null;
  record: PythonCodeRunRecord | null;
  running: boolean;
  width?: number;
  onClose: () => void;
  onRun: (target: PythonCodeRunTarget) => void;
  onStop: (runId: string) => void;
  onClear: (target: PythonCodeRunTarget) => void;
}

export const PythonRunDrawer = memo(function PythonRunDrawer({
  target,
  record,
  running,
  width = 420,
  onClose,
  onRun,
  onStop,
  onClear,
}: PythonRunDrawerProps) {
  const installedPackages = useMemo(
    () => parseJsonArray<string>(record?.installedPackagesJson),
    [record?.installedPackagesJson],
  );
  const toolEvents = useMemo(
    () => parseJsonArray<PythonRunToolEvent>(record?.toolEventsJson),
    [record?.toolEventsJson],
  );
  const statusLabel = record ? statusText(record.status) : "未运行";
  const canRun = Boolean(target) && !running;

  return (
    <aside style={{ ...styles.drawer, width }}>
      <div style={styles.header}>
        <div style={styles.titleWrap}>
          <span style={styles.kicker}>Python Runner</span>
          <strong style={styles.title}>{statusLabel}</strong>
        </div>
        <button type="button" style={styles.iconButton} onClick={onClose} title="关闭">
          <X size={16} />
        </button>
      </div>

      <div style={styles.notice}>
        使用全应用共享 uv 虚拟环境执行。依赖会自动安装并保留，适合连续学习，但结果会受已安装包影响。
      </div>

      <div style={styles.actions}>
        <button
          type="button"
          style={{ ...styles.actionButton, opacity: canRun ? 1 : 0.55 }}
          disabled={!canRun || !target}
          onClick={() => target && onRun(target)}
        >
          <RotateCcw size={14} />
          {record ? "重新运行" : "运行"}
        </button>
        <button
          type="button"
          style={{ ...styles.actionButton, opacity: running && record ? 1 : 0.55 }}
          disabled={!running || !record}
          onClick={() => record && onStop(record.runId)}
        >
          <Square size={13} />
          停止
        </button>
        <button
          type="button"
          style={{ ...styles.actionButton, opacity: target && record && !running ? 1 : 0.55 }}
          disabled={!target || !record || running}
          onClick={() => target && onClear(target)}
        >
          <Trash2 size={13} />
          清空
        </button>
      </div>

      <div style={styles.body}>
        {target ? (
          <>
            <Section title={`代码块 #${target.codeBlockIndex + 1}`}>
              <pre style={styles.codePreview}>{target.code}</pre>
            </Section>

            {record?.errorReason && (
              <Section title="错误原因">
                <div style={styles.errorText}>{record.errorReason}</div>
              </Section>
            )}

            <Section title="stdout">
              <pre style={styles.output}>{record?.stdout || "暂无输出"}</pre>
            </Section>

            <Section title="stderr">
              <pre style={styles.output}>{record?.stderr || "暂无错误输出"}</pre>
            </Section>

            {installedPackages.length > 0 && (
              <Section title="已安装依赖">
                <div style={styles.chipRow}>
                  {installedPackages.map((pkg) => (
                    <span key={pkg} style={styles.chip}>
                      {pkg}
                    </span>
                  ))}
                </div>
              </Section>
            )}

            {toolEvents.length > 0 && (
              <Section title="执行步骤">
                <div style={styles.timeline}>
                  {toolEvents.map((event, index) => (
                    <div key={`${event.createdAt}-${index}`} style={styles.timelineItem}>
                      <strong>{event.name}</strong>
                      <span>{event.detail}</span>
                    </div>
                  ))}
                </div>
              </Section>
            )}

            <Section title="教学解释">
              {record?.explanationMarkdown ? (
                <MarkdownRenderer content={record.explanationMarkdown} variant="chat" />
              ) : (
                <div style={styles.muted}>运行完成后会显示解释。</div>
              )}
            </Section>
          </>
        ) : (
          <div style={styles.empty}>点击 Python 代码块右上角 Run 查看执行结果。</div>
        )}
      </div>
    </aside>
  );
});

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section style={styles.section}>
      <div style={styles.sectionTitle}>{title}</div>
      {children}
    </section>
  );
}

function parseJsonArray<T>(raw?: string | null): T[] {
  if (!raw) return [];
  try {
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

function statusText(status: string) {
  switch (status) {
    case "running":
      return "运行中";
    case "done":
      return "已完成";
    case "failed":
      return "执行失败";
    case "stopped":
      return "已停止";
    default:
      return status || "未运行";
  }
}

const styles = {
  drawer: {
    flexShrink: 0,
    minWidth: 340,
    maxWidth: "48vw",
    display: "flex",
    flexDirection: "column",
    borderLeft: "1px solid var(--border-dim)",
    background: "var(--bg-panel)",
    color: "var(--text-primary)",
    overflow: "hidden",
  },
  header: {
    height: 54,
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: 10,
    padding: "0 14px",
    borderBottom: "1px solid var(--border-dim)",
    background: "var(--bg-sidebar)",
  },
  titleWrap: { display: "flex", flexDirection: "column", gap: 2, minWidth: 0 },
  kicker: { fontSize: 10, textTransform: "uppercase", color: "var(--text-muted)", fontWeight: 700 },
  title: { fontSize: 14, color: "var(--text-primary)" },
  iconButton: {
    width: 28,
    height: 28,
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    border: "1px solid var(--border-dim)",
    borderRadius: 6,
    background: "var(--bg-card)",
    color: "var(--text-muted)",
    cursor: "pointer",
  },
  notice: {
    padding: "10px 14px",
    borderBottom: "1px solid var(--border-dim)",
    fontSize: 12,
    lineHeight: 1.55,
    color: "var(--text-muted)",
    background: "var(--bg-subtle)",
  },
  actions: {
    display: "flex",
    gap: 8,
    padding: 12,
    borderBottom: "1px solid var(--border-dim)",
  },
  actionButton: {
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    gap: 6,
    padding: "7px 10px",
    borderRadius: 6,
    border: "1px solid var(--border-dim)",
    background: "var(--bg-card)",
    color: "var(--text-primary)",
    fontSize: 12,
    fontWeight: 650,
    cursor: "pointer",
  },
  body: { flex: 1, minHeight: 0, overflow: "auto", padding: 14 },
  section: { marginBottom: 16 },
  sectionTitle: {
    marginBottom: 8,
    color: "var(--text-muted)",
    fontSize: 11,
    fontWeight: 800,
    textTransform: "uppercase",
  },
  codePreview: {
    margin: 0,
    padding: 12,
    maxHeight: 180,
    overflow: "auto",
    borderRadius: 8,
    border: "1px solid var(--border-dim)",
    background: "var(--markdown-code-bg)",
    color: "var(--markdown-code-text)",
    fontFamily: "var(--font-mono)",
    fontSize: 12,
    lineHeight: 1.55,
  },
  output: {
    margin: 0,
    minHeight: 42,
    maxHeight: 180,
    overflow: "auto",
    whiteSpace: "pre-wrap",
    wordBreak: "break-word",
    padding: 12,
    borderRadius: 8,
    border: "1px solid var(--border-dim)",
    background: "var(--bg-card)",
    color: "var(--text-primary)",
    fontFamily: "var(--font-mono)",
    fontSize: 12,
    lineHeight: 1.55,
  },
  errorText: {
    padding: 12,
    borderRadius: 8,
    border: "1px solid color-mix(in srgb, var(--danger) 35%, var(--border-dim))",
    color: "var(--danger)",
    background: "color-mix(in srgb, var(--danger) 8%, var(--bg-card))",
    fontSize: 12,
    lineHeight: 1.55,
  },
  chipRow: { display: "flex", flexWrap: "wrap", gap: 6 },
  chip: {
    padding: "4px 7px",
    borderRadius: 6,
    border: "1px solid var(--border-dim)",
    background: "var(--bg-card)",
    fontFamily: "var(--font-mono)",
    fontSize: 11,
  },
  timeline: { display: "flex", flexDirection: "column", gap: 8 },
  timelineItem: {
    display: "flex",
    flexDirection: "column",
    gap: 4,
    padding: 10,
    borderRadius: 8,
    border: "1px solid var(--border-dim)",
    background: "var(--bg-card)",
    fontSize: 12,
    lineHeight: 1.45,
    whiteSpace: "pre-wrap",
  },
  muted: { color: "var(--text-muted)", fontSize: 13 },
  empty: {
    height: "100%",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    textAlign: "center",
    color: "var(--text-muted)",
    fontSize: 13,
    lineHeight: 1.6,
    padding: 24,
  },
} satisfies Record<string, React.CSSProperties>;

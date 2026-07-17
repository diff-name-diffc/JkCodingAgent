import { memo, useMemo } from "react";
import type React from "react";
import { RotateCcw, Square, Trash2, X } from "lucide-react";
import type { PythonCodeRunRecord, PythonCodeRunTarget, PythonRunToolEvent } from "../../types";
import { MarkdownRenderer } from "../markdown/MarkdownRenderer";
import { IconButton } from "../IconButton";

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
    <aside className="ai-python-run-drawer ai-migrated-python-runner" style={{ width }}>
      <div className="ai-python-run-header">
        <div className="ai-python-run-title-wrap">
          <span className="ai-python-run-kicker">Python Runner</span>
          <strong className="ai-python-run-title">{statusLabel}</strong>
        </div>
        <IconButton icon={<X size={16} />} title="关闭" onClick={onClose} size={28} />
      </div>

      <div className="ai-python-run-notice">
        使用全应用共享 uv 虚拟环境执行。依赖会自动安装并保留，适合连续学习，但结果会受已安装包影响。
      </div>

      <div className="ai-python-run-actions">
        <button
          type="button"
          className="ai-python-run-action"
          style={{ opacity: canRun ? 1 : 0.55 }}
          disabled={!canRun || !target}
          onClick={() => target && onRun(target)}
        >
          <RotateCcw size={14} />
          {record ? "重新运行" : "运行"}
        </button>
        <button
          type="button"
          className="ai-python-run-action"
          style={{ opacity: running && record ? 1 : 0.55 }}
          disabled={!running || !record}
          onClick={() => record && onStop(record.runId)}
        >
          <Square size={13} />
          停止
        </button>
        <button
          type="button"
          className="ai-python-run-action"
          style={{ opacity: target && record && !running ? 1 : 0.55 }}
          disabled={!target || !record || running}
          onClick={() => target && onClear(target)}
        >
          <Trash2 size={13} />
          清空
        </button>
      </div>

      <div className="ai-python-run-body">
        {target ? (
          <>
            <Section title={`代码块 #${target.codeBlockIndex + 1}`}>
              <pre className="ai-python-run-code">{target.code}</pre>
            </Section>

            {record?.errorReason && (
              <Section title="错误原因">
                <div className="ai-python-run-error">{record.errorReason}</div>
              </Section>
            )}

            <Section title="stdout">
              <pre className="ai-python-run-output">{record?.stdout || "暂无输出"}</pre>
            </Section>

            <Section title="stderr">
              <pre className="ai-python-run-output">{record?.stderr || "暂无错误输出"}</pre>
            </Section>

            {installedPackages.length > 0 && (
              <Section title="已安装依赖">
                <div className="ai-python-run-chip-row">
                  {installedPackages.map((pkg) => (
                    <span key={pkg} className="ai-python-run-chip">
                      {pkg}
                    </span>
                  ))}
                </div>
              </Section>
            )}

            {toolEvents.length > 0 && (
              <Section title="执行步骤">
                <div className="ai-python-run-timeline">
                  {toolEvents.map((event, index) => (
                    <div key={`${event.createdAt}-${index}`} className="ai-python-run-timeline-item">
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
                <div className="ai-python-run-muted">运行完成后会显示解释。</div>
              )}
            </Section>
          </>
        ) : (
          <div className="ai-python-run-empty">点击 Python 代码块右上角 Run 查看执行结果。</div>
        )}
      </div>
    </aside>
  );
});

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="ai-python-run-section">
      <div className="ai-python-run-section-title">{title}</div>
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

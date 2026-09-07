import { memo, useEffect, useMemo, useState } from "react";
import { RotateCcw, Square, Trash2, X } from "lucide-react";
import type { PythonCodeRunRecord, PythonCodeRunTarget, PythonRunToolEvent } from "../../types";
import { MarkdownRenderer } from "../markdown/MarkdownRenderer";
import { IconButton } from "../IconButton";
import { ActivityTimeline } from "../detail/ActivityTimeline";
import { DetailSection } from "../detail/DetailSection";
import { OutputBlock } from "../detail/OutputBlock";
import { StatusPill } from "../detail/StatusPill";
import {
  formatPythonRunDuration,
  formatPythonRunStartedAt,
  pythonRunSourceLabel,
} from "./python-run-meta";

interface PythonRunDrawerProps {
  target: PythonCodeRunTarget | null;
  record: PythonCodeRunRecord | null;
  running: boolean;
  onClose: () => void;
  onRun: (target: PythonCodeRunTarget) => void;
  onStop: (runId: string) => void;
  onClear: (target: PythonCodeRunTarget) => void;
}

/**
 * Python 运行详情（UI-20）：状态/耗时/来源统一头部，状态视觉与代码块
 * 内联徽章同源（StatusPill domain="python"）。运行中耗时以 1s 间隔驱动，
 * interval 仅在本组件（抽屉打开且 running）内挂载，不触发全局重渲染。
 * 记录无 exitCode 字段：失败反馈由 status + 错误原因承担（不虚构退出码）。
 */
export const PythonRunDrawer = memo(function PythonRunDrawer({
  target,
  record,
  running,
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
  const canRun = Boolean(target) && !running;

  // 运行中每 1s 推进耗时读数；终态直接用 updatedAt−createdAt（纯函数复算）。
  const [nowMs, setNowMs] = useState(() => Date.now());
  useEffect(() => {
    if (!running || !record) return;
    const timer = window.setInterval(() => setNowMs(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [running, record]);
  const duration = record ? formatPythonRunDuration(record, nowMs) : null;
  const startedAt = record ? formatPythonRunStartedAt(record) : null;
  const source = record ?? target;

  const timelineRows = useMemo(
    () =>
      toolEvents.map((event, index) => ({
        id: `${event.createdAt}-${index}`,
        status: "neutral" as const,
        label: event.name,
        detail: event.detail || undefined,
        meta: formatPythonRunStartedAt({ createdAt: event.createdAt }) ?? undefined,
      })),
    [toolEvents],
  );

  return (
    <div className="ai-python-run-body">
      <div className="ai-python-run-header">
        <div className="ai-python-run-title-wrap">
          <span className="ai-python-run-kicker">Python Runner</span>
          <StatusPill domain="python" status={record ? record.status : "idle"} />
          {duration && <span className="ai-python-run-duration">耗时 {duration}</span>}
        </div>
        <IconButton icon={<X size={16} />} title="关闭" onClick={onClose} size={28} />
      </div>

      {source && (
        <div className="ai-python-run-meta">
          <span>{pythonRunSourceLabel(source)}</span>
          {startedAt && <span>开始于 {startedAt}</span>}
        </div>
      )}

      <div className="ai-python-run-notice">
        使用全应用共享 uv 虚拟环境执行。依赖会自动安装并保留，适合连续学习，但结果会受已安装包影响。
      </div>

      <div className="ai-python-run-actions">
        <button
          type="button"
          className="ai-python-run-action"
          disabled={!canRun || !target}
          onClick={() => target && onRun(target)}
        >
          <RotateCcw size={14} />
          {record ? "重新运行" : "运行"}
        </button>
        <button
          type="button"
          className="ai-python-run-action"
          disabled={!running || !record}
          onClick={() => record && onStop(record.runId)}
        >
          <Square size={13} />
          停止
        </button>
        <button
          type="button"
          className="ai-python-run-action"
          disabled={!target || !record || running}
          onClick={() => target && onClear(target)}
        >
          <Trash2 size={13} />
          清空
        </button>
      </div>

      <div className="ai-python-run-body ai-python-run-content">
        {target ? (
          <>
            <DetailSection title={`代码块 #${target.codeBlockIndex + 1}`}>
              <pre className="ai-python-run-code chat-scroll">{target.code}</pre>
            </DetailSection>

            {record?.errorReason && (
              <DetailSection title="错误原因">
                <OutputBlock text={record.errorReason} tone="error" emptyHint="无错误信息" />
              </DetailSection>
            )}

            <DetailSection title="标准输出">
              <pre className="ai-python-run-output chat-scroll">{record?.stdout || "暂无输出"}</pre>
            </DetailSection>

            <DetailSection title="标准错误">
              <pre className="ai-python-run-output chat-scroll">
                {record?.stderr || "暂无错误输出"}
              </pre>
            </DetailSection>

            {installedPackages.length > 0 && (
              <DetailSection title="已安装依赖">
                <div className="ai-python-run-chip-row">
                  {installedPackages.map((pkg) => (
                    <span key={pkg} className="ai-python-run-chip">
                      {pkg}
                    </span>
                  ))}
                </div>
              </DetailSection>
            )}

            {toolEvents.length > 0 && (
              <DetailSection title="执行步骤">
                <ActivityTimeline rows={timelineRows} />
              </DetailSection>
            )}

            <DetailSection title="教学解释">
              {record?.explanationMarkdown ? (
                <MarkdownRenderer content={record.explanationMarkdown} variant="chat" />
              ) : (
                <div className="ai-python-run-muted">运行完成后会显示解释。</div>
              )}
            </DetailSection>
          </>
        ) : (
          <div className="ai-python-run-empty">点击 Python 代码块右上角 Run 查看执行结果。</div>
        )}
      </div>
    </div>
  );
});

function parseJsonArray<T>(raw?: string | null): T[] {
  if (!raw) return [];
  try {
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

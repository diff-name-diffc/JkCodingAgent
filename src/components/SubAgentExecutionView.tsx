import { useEffect, useState } from "react";
import {
  Bot,
  ChevronDown,
  ChevronRight,
  LoaderCircle,
  XCircle,
  Zap,
  Sparkles,
  Wrench,
  PenLine,
} from "lucide-react";
import type { SubAgentSession, SubAgentPhase, SubAgentToolCall } from "./subAgentEventStore";
import { formatTokenGenerationSpeed } from "./dispatcher-chat/dispatcherChatUtils";
import { MarkdownRenderer } from "./markdown/MarkdownRenderer";
import { ActivityTimeline, type ActivityRow } from "./detail/ActivityTimeline";
import { DetailSection } from "./detail/DetailSection";
import { OutputBlock } from "./detail/OutputBlock";
import { StatusPill } from "./detail/StatusPill";

// ── Phase Metadata ──────────────────────────────────────────────────────────

const PHASE_STEPS: { key: SubAgentPhase; label: string; Icon: typeof Zap }[] = [
  { key: "initializing", label: "启动", Icon: Zap },
  { key: "thinking", label: "思考", Icon: Sparkles },
  { key: "tool_calling", label: "工具调用", Icon: Wrench },
  { key: "generating", label: "生成结果", Icon: PenLine },
];

const PHASE_ORDER: Record<SubAgentPhase, number> = {
  initializing: 0,
  thinking: 1,
  tool_calling: 2,
  generating: 3,
  completed: 4,
  failed: -1,
};

const PHASE_LABEL: Record<SubAgentPhase, string> = {
  initializing: "初始化",
  thinking: "思考中",
  tool_calling: "调用工具",
  generating: "生成结果",
  completed: "已完成",
  failed: "已失败",
};

// ── Helpers ───────────────────────────────────────────────────────────────────

function formatElapsed(ms: number): string {
  const secs = Math.round(ms / 1000);
  return secs >= 60 ? `${Math.floor(secs / 60)}m ${secs % 60}s` : `${secs}s`;
}

function formatArgsPreview(args: Record<string, unknown>): string {
  const str = JSON.stringify(args);
  if (!str || str === "{}") return "()";
  return `(${str.slice(1, 81)}${str.length > 82 ? "..." : ""})`;
}

// ── Elapsed Timer Hook ────────────────────────────────────────────────────────

function useLiveElapsed(session: SubAgentSession): number {
  const [elapsed, setElapsed] = useState(session.elapsed);
  useEffect(() => {
    if (session.status !== "running") {
      setElapsed(session.elapsed);
      return;
    }
    const base = session.elapsed;
    const startAt = Date.now();
    const id = setInterval(() => setElapsed(base + (Date.now() - startAt)), 1000);
    return () => clearInterval(id);
  }, [session.status, session.elapsed]);
  return elapsed;
}

// ── PhaseIndicator ────────────────────────────────────────────────────────────

function PhaseIndicator({ phase }: { phase: SubAgentPhase }) {
  const currentIdx = PHASE_ORDER[phase];
  const isFailed = phase === "failed";

  return (
    <div className="ai-subagent-exec-phase-bar">
      {PHASE_STEPS.map((step, idx) => {
        const done = !isFailed && currentIdx > idx;
        const active = !isFailed && currentIdx === idx;
        const failedHere = isFailed && idx === PHASE_STEPS.length - 1;

        const stepClass = failedHere
          ? "ai-subagent-exec-phase-step is-failed"
          : done
            ? "ai-subagent-exec-phase-step is-done"
            : active
              ? "ai-subagent-exec-phase-step is-active"
              : "ai-subagent-exec-phase-step";

        const StepIcon = failedHere ? XCircle : step.Icon;

        return (
          <span key={step.key} className="ai-subagent-exec-phase-step-wrap" style={{ flex: idx < PHASE_STEPS.length - 1 ? 1 : undefined }}>
            <span className={stepClass}>
              <StepIcon size={11} />
              {step.label}
            </span>
            {idx < PHASE_STEPS.length - 1 && (
              <span className={done ? "ai-subagent-exec-phase-connector is-done" : "ai-subagent-exec-phase-connector"} />
            )}
          </span>
        );
      })}
    </div>
  );
}

// ── 活动时间线（共享 ActivityTimeline 映射） ─────────────────────────────────

function isCommandAuditPreview(toolCall: SubAgentToolCall): boolean {
  const preview = toolCall.resultPreview ?? "";
  return (
    (toolCall.toolName === "ssh_exec" && preview.startsWith("## SSH 命令审查记录")) ||
    (toolCall.toolName === "local_zsh" &&
      preview.startsWith("## local_zsh 执行结果") &&
      preview.includes("审查结论: `拦截`"))
  );
}

function buildActivityRows(toolCalls: SubAgentToolCall[]): ActivityRow[] {
  return toolCalls.map((tc) => {
    const isRunning = tc.status === "running";
    const isFailed = tc.status === "failed";
    return {
      id: tc.id,
      status: isFailed ? "error" : isRunning ? "running" : "success",
      label: (
        <>
          {tc.toolName}
          {isRunning && (
            <LoaderCircle size={10} className="spin" style={{ marginLeft: 4, verticalAlign: "middle" }} />
          )}
        </>
      ),
      meta: isRunning ? (
        "执行中..."
      ) : (
        <>
          {isFailed ? "失败" : "成功"}
          {tc.durationMs != null && ` · ${(tc.durationMs / 1000).toFixed(1)}s`}
        </>
      ),
      detail: <span className="ai-activity-timeline-args">{formatArgsPreview(tc.arguments)}</span>,
      children:
        tc.resultPreview && !isRunning ? (
          isCommandAuditPreview(tc) ? (
            <div className="ai-activity-timeline-audit">
              <MarkdownRenderer content={tc.resultPreview} variant="chat" />
            </div>
          ) : (
            <div className="ai-activity-timeline-preview">
              {tc.resultPreview.slice(0, 120)}
              {tc.resultPreview.length > 120 ? "..." : ""}
            </div>
          )
        ) : undefined,
    };
  });
}

// ── SubAgentExecutionCard (main export) ───────────────────────────────────────

interface SubAgentExecutionCardProps {
  session: SubAgentSession;
  autoExpand?: boolean;
}

const TASK_PREVIEW_LIMIT = 200;

/**
 * 子智能体执行详情（UI-14 统一为「概览 / 活动 / 输出」三段）：
 * 与节点抽屉、产物详情共享 DetailSection / ActivityTimeline / OutputBlock /
 * StatusPill 视觉。实时事件与历史轨迹重放走同一渲染路径（数据链不动）。
 * 概览显示来源任务、模型、耗时与错误——模型来自运行记录（Started 事件 /
 * trace 表 model 列，UI-14 遗留已补），老轨迹两源皆无时如实显示「未记录」，
 * 不用当前配置冒充运行记录。
 */
export function SubAgentExecutionCard({ session, autoExpand = true }: SubAgentExecutionCardProps) {
  const [isOpen, setIsOpen] = useState(autoExpand);
  const [taskExpanded, setTaskExpanded] = useState(false);
  const elapsed = useLiveElapsed(session);

  const phaseLabel = PHASE_LABEL[session.phase];
  const isActive = session.status === "running";
  const task = session.task ?? "";
  const taskLong = task.length > TASK_PREVIEW_LIMIT;
  const taskText = taskLong && !taskExpanded ? `${task.slice(0, TASK_PREVIEW_LIMIT)}...` : task;
  const completionTokens = session.tokenUsage?.completionTokens ?? 0;
  const speed = isActive && completionTokens > 0 ? formatTokenGenerationSpeed(completionTokens, elapsed) : null;

  return (
    <div className="ai-subagent-exec ai-migrated-tool-activity">
      {/* Header */}
      <button type="button" onClick={() => setIsOpen((prev) => !prev)} className="ai-subagent-exec-header">
        {isOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        <Bot size={14} className="ai-subagent-exec-icon" />
        <span className="ai-subagent-exec-name">子智能体：{session.name}</span>

        {/* Phase label chip */}
        <span className={isActive ? "ai-subagent-exec-phase is-active" : "ai-subagent-exec-phase"}>
          {isActive && session.phase === "tool_calling" ? (
            <LoaderCircle size={10} className="spin" style={{ marginRight: 3, verticalAlign: "middle" }} />
          ) : null}
          {phaseLabel}
        </span>

        <span className="ai-subagent-exec-elapsed">{formatElapsed(elapsed)}</span>
        <StatusPill domain="subagent" status={session.status} />
      </button>

      {/* Expanded body：概览 / 活动 / 输出 */}
      {isOpen && (
        <div className="ai-subagent-exec-body">
          <DetailSection title="概览">
            <div className="ai-detail-meta-grid">
              <div className="ai-detail-meta-item">
                <span className="ai-detail-meta-label">耗时</span>
                <span className="ai-detail-meta-value">{formatElapsed(elapsed)}</span>
              </div>
              <div className="ai-detail-meta-item">
                <span className="ai-detail-meta-label">模型</span>
                {session.model ? (
                  <span
                    className="ai-detail-meta-value"
                    title={`运行实际使用的模型：${session.model}`}
                  >
                    {session.model}
                  </span>
                ) : (
                  <span
                    className="ai-detail-meta-value ai-detail-meta-value--muted"
                    title="该任务执行轨迹未记录模型信息"
                  >
                    未记录
                  </span>
                )}
              </div>
              {speed && (
                <div className="ai-detail-meta-item">
                  <span className="ai-detail-meta-label">速度</span>
                  <span className="ai-detail-meta-value">{speed} t/s</span>
                </div>
              )}
              {!isActive && session.iterations != null && (
                <div className="ai-detail-meta-item">
                  <span className="ai-detail-meta-label">迭代</span>
                  <span className="ai-detail-meta-value">{session.iterations} 轮</span>
                </div>
              )}
              {!isActive && session.tokenUsage?.totalTokens != null && (
                <div className="ai-detail-meta-item">
                  <span className="ai-detail-meta-label">Token 用量</span>
                  <span className="ai-detail-meta-value">
                    {session.tokenUsage.totalTokens.toLocaleString()}
                  </span>
                </div>
              )}
            </div>
            {task && (
              <div className="ai-subagent-exec-task">
                <span className="ai-detail-meta-label">来源任务</span>
                <div className="ai-subagent-exec-task-text">{taskText}</div>
                {taskLong && (
                  <button
                    type="button"
                    className="ai-subagent-exec-task-toggle"
                    onClick={() => setTaskExpanded((value) => !value)}
                    aria-expanded={taskExpanded}
                  >
                    {taskExpanded ? "收起" : "展开全文"}
                  </button>
                )}
              </div>
            )}
            {session.status === "failed" && session.finishedError && (
              <OutputBlock text={session.finishedError} tone="error" emptyHint="无错误信息" />
            )}
          </DetailSection>

          <DetailSection title="活动">
            <PhaseIndicator phase={session.phase} />
            {/* Progress messages (running state only) */}
            {isActive && session.progressMessages.length > 0 && (
              <div className="ai-subagent-exec-progress">
                {session.progressMessages.slice(-5).map((msg) => (
                  <div key={msg.id} className="ai-subagent-exec-progress-item">{msg.text}</div>
                ))}
              </div>
            )}
            <ActivityTimeline rows={buildActivityRows(session.toolCalls)} />
            {session.toolCalls.length === 0 && (
              <p className="ai-subagent-exec-empty">
                {isActive ? "尚未开始工具调用…" : "该任务没有记录工具调用。"}
              </p>
            )}
          </DetailSection>

          <DetailSection title="输出">
            <OutputBlock
              text={session.status === "completed" ? (session.finishedResult ?? "") : ""}
              emptyHint={isActive ? "正在生成结果…" : "无最终结果"}
              className="session-selectable"
            />
          </DetailSection>
        </div>
      )}
    </div>
  );
}

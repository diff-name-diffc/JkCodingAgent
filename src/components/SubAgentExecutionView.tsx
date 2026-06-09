import { useEffect, useState } from "react";
import {
  Bot,
  ChevronDown,
  ChevronRight,
  LoaderCircle,
  XCircle,
  Check,
  Zap,
  Sparkles,
  Wrench,
  PenLine,
  AlertTriangle,
  Clock,
  Coins,
  RotateCcw,
} from "lucide-react";
import type { SubAgentSession, SubAgentPhase, SubAgentToolCall } from "./subAgentEventStore";
import s from "../styles";

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
    <div style={s.phaseBar}>
      {PHASE_STEPS.map((step, idx) => {
        const done = !isFailed && currentIdx > idx;
        const active = !isFailed && currentIdx === idx;
        const failedHere = isFailed && idx === PHASE_STEPS.length - 1;

        const stepStyle = failedHere
          ? s.phaseStepFailed
          : done
            ? s.phaseStepDone
            : active
              ? s.phaseStepActive
              : s.phaseStep;

        const StepIcon = failedHere ? XCircle : step.Icon;

        return (
          <span key={step.key} style={{ display: "flex", alignItems: "center", flex: idx < PHASE_STEPS.length - 1 ? 1 : undefined }}>
            <span style={stepStyle}>
              <StepIcon size={11} />
              {step.label}
            </span>
            {idx < PHASE_STEPS.length - 1 && (
              <span style={done ? s.phaseConnectorDone : s.phaseConnector} />
            )}
          </span>
        );
      })}
    </div>
  );
}

// ── StatsBar ──────────────────────────────────────────────────────────────────

function StatsBar({ session }: { session: SubAgentSession }) {
  if (session.status !== "completed" && session.status !== "failed") return null;

  return (
    <div style={s.statsBar}>
      {session.iterations != null && (
        <span style={s.statsChip}>
          <RotateCcw size={10} />
          {session.iterations} 轮
        </span>
      )}
      {session.tokenUsage?.totalTokens != null && (
        <span style={s.statsChip}>
          <Coins size={10} />
          {session.tokenUsage.totalTokens.toLocaleString()} tokens
        </span>
      )}
      <span style={s.statsChip}>
        <Clock size={10} />
        {formatElapsed(session.elapsed)}
      </span>
    </div>
  );
}

// ── ToolCallTimeline ──────────────────────────────────────────────────────────

function ToolCallTimeline({ toolCalls }: { toolCalls: SubAgentToolCall[] }) {
  if (toolCalls.length === 0) return null;

  return (
    <div style={s.timeline}>
      {toolCalls.map((tc, idx) => {
        const isLast = idx === toolCalls.length - 1;
        const isRunning = tc.status === "running";
        const isFailed = tc.status === "failed";

        return (
          <div key={tc.id} style={s.timelineItem}>
            {/* Vertical connecting line (not on last item) */}
            {!isLast && <span style={s.timelineLine} />}

            {/* Dot */}
            <span
              style={
                isFailed
                  ? s.timelineDotFailed
                  : isRunning
                    ? s.timelineDotRunning
                    : s.timelineDot
              }
            />

            {/* Content */}
            <div>
              <span style={s.timelineToolName}>
                {tc.toolName}
                {isRunning && (
                  <LoaderCircle size={10} className="spin" style={{ marginLeft: 4, verticalAlign: "middle" }} />
                )}
              </span>
              <span style={{ fontSize: 10.5, color: "var(--text-muted)", fontFamily: "var(--font-mono)" }}>
                {" "}{formatArgsPreview(tc.arguments)}
              </span>
              <div style={s.timelineMeta}>
                {isRunning ? (
                  "执行中..."
                ) : (
                  <>
                    {isFailed ? "失败" : "成功"}
                    {tc.durationMs != null && ` · ${(tc.durationMs / 1000).toFixed(1)}s`}
                  </>
                )}
              </div>
              {tc.resultPreview && !isRunning && (
                <div style={s.timelineResultPreview}>
                  {tc.resultPreview.slice(0, 120)}
                  {(tc.resultPreview.length > 120) ? "..." : ""}
                </div>
              )}
            </div>
          </div>
        );
      })}
    </div>
  );
}

// ── ResultBlock ───────────────────────────────────────────────────────────────

function ResultBlock({ result }: { result: string }) {
  if (!result.trim()) return null;
  return (
    <div>
      <div style={s.resultLabel}>最终结果</div>
      <pre className="session-selectable" style={s.resultBlock}>{result}</pre>
    </div>
  );
}

// ── ErrorBlock ────────────────────────────────────────────────────────────────

function ErrorBlock({ error }: { error: string }) {
  if (!error.trim()) return null;
  return (
    <div>
      <div style={s.errorLabel}>执行失败</div>
      <div style={s.errorBlock}>
        <AlertTriangle size={14} style={{ flexShrink: 0, marginTop: 2 }} />
        <span>{error}</span>
      </div>
    </div>
  );
}

// ── SubAgentExecutionCard (main export) ───────────────────────────────────────

interface SubAgentExecutionCardProps {
  session: SubAgentSession;
  autoExpand?: boolean;
}

export function SubAgentExecutionCard({ session, autoExpand = true }: SubAgentExecutionCardProps) {
  const [isOpen, setIsOpen] = useState(autoExpand);
  const elapsed = useLiveElapsed(session);

  const statusColor =
    session.status === "running"
      ? "var(--accent)"
      : session.status === "failed"
        ? "var(--danger, #ef4444)"
        : "var(--success, #22c55e)";

  const StatusIcon =
    session.status === "running"
      ? LoaderCircle
      : session.status === "failed"
        ? XCircle
        : Check;

  const phaseLabel = PHASE_LABEL[session.phase];
  const isActive = session.status === "running";

  return (
    <div style={s.card}>
      {/* Header */}
      <button type="button" onClick={() => setIsOpen((prev) => !prev)} style={s.cardHeader}>
        {isOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        <Bot size={14} style={{ color: statusColor, flexShrink: 0 }} />
        <span style={s.headerName}>子智能体：{session.name}</span>

        {/* Phase label chip */}
        <span style={isActive ? s.headerPhaseLabelActive : s.headerPhaseLabel}>
          {isActive && session.phase === "tool_calling" ? (
            <LoaderCircle size={10} className="spin" style={{ marginRight: 3, verticalAlign: "middle" }} />
          ) : null}
          {phaseLabel}
        </span>

        {/* Elapsed */}
        <span style={s.headerElapsed}>{formatElapsed(elapsed)}</span>

        {/* Status icon */}
        <StatusIcon size={12} style={{ color: statusColor }} className={session.status === "running" ? "spin" : ""} />
      </button>

      {/* Expanded body */}
      {isOpen && (
        <div style={s.cardBody}>
          {/* Phase indicator bar */}
          <PhaseIndicator phase={session.phase} />

          {/* Task description */}
          {session.task && (
            <div style={s.taskDescription}>
              任务：{session.task.slice(0, 200)}
              {session.task.length > 200 ? "..." : ""}
            </div>
          )}

          {/* Stats bar (completed/failed only) */}
          <StatsBar session={session} />

          {/* Tool call timeline */}
          <ToolCallTimeline toolCalls={session.toolCalls} />

          {/* Progress messages (running state only, when no tool calls active) */}
          {session.status === "running" && session.progressMessages.length > 0 && (
            <div style={s.progressRow}>
              {session.progressMessages.slice(-5).map((msg) => (
                <div key={msg.id} style={s.progressItem}>{msg.text}</div>
              ))}
            </div>
          )}

          {/* Final result */}
          {session.status === "completed" && session.finishedResult && (
            <ResultBlock result={session.finishedResult} />
          )}

          {/* Error details */}
          {session.status === "failed" && session.finishedError && (
            <ErrorBlock error={session.finishedError} />
          )}
        </div>
      )}
    </div>
  );
}

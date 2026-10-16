import { useEffect, useState, type ReactNode } from "react";
import { ArrowDown, ArrowUp, Gauge, Zap } from "lucide-react";
import type { DispatcherMessageUsageStats } from "../../types";
import { cn } from "../../lib/cn";
import {
  getDispatcherLiveSessionState,
  subscribeDispatcherLiveSession,
} from "../dispatcherSessionStore";
import {
  formatTokenCount,
  formatTokenGenerationSpeed,
} from "../dispatcher-chat/dispatcherChatUtils";

/** 上下文占用快照（最近一次请求的 prompt tokens / 窗口容量）。 */
export interface ContextUsageSnapshot {
  usedTokens: number;
  capacityTokens: number;
}

/** 会话统计条的静态数据（页面层按会话 memo）：累计输入/输出、最近一轮用量
 * 与上下文占用快照。 */
export interface SessionStatsData {
  /** 会话累计输入 tokens（primary 口径，跨模型求和）。 */
  inputTokens: number;
  /** 会话累计输出 tokens（primary 口径，跨模型求和）。 */
  outputTokens: number;
  /** 最近一次已完成 run 的用量快照（来自 assistant 消息 usageStats），
   * live 快照缺位（未在运行 / run 已收尾对账）时作为速度回退。 */
  lastRunStats: DispatcherMessageUsageStats | null;
  /** 最近一次请求的上下文占用（容量回路闭环；无数据时为 null）。 */
  contextUsage: ContextUsageSnapshot | null;
}

export interface SessionStatsBarProps {
  sessionId: string | null;
  stats: SessionStatsData;
}

/** 窄订阅 live state 的 runUsage：引用比较兜底，runUsage 未变的流式帧
 * 不触发本组件重渲染（订阅回调每 rAF 合帧都会到，但快照对象只在每次
 * 模型调用完成的 runUsageUpdated 时更换）。 */
function useLiveRunUsage(sessionId: string | null): DispatcherMessageUsageStats | null {
  const [snapshot, setSnapshot] = useState<{
    id: string | null;
    usage: DispatcherMessageUsageStats | null;
  }>(() => ({
    id: sessionId,
    usage: sessionId ? (getDispatcherLiveSessionState(sessionId)?.runUsage ?? null) : null,
  }));

  // 渲染期调整（同 useLiveSessionStateReadonly）：会话切换首帧即读新会话快照。
  if (snapshot.id !== sessionId) {
    setSnapshot({
      id: sessionId,
      usage: sessionId ? (getDispatcherLiveSessionState(sessionId)?.runUsage ?? null) : null,
    });
  }

  useEffect(() => {
    if (!sessionId) return;
    const align = () => {
      const current = getDispatcherLiveSessionState(sessionId)?.runUsage ?? null;
      setSnapshot((previous) =>
        previous.id === sessionId && previous.usage === current
          ? previous
          : { id: sessionId, usage: current },
      );
    };
    const unsubscribe = subscribeDispatcherLiveSession(sessionId, align);
    // 订阅后对齐一次，兜住渲染期读取与订阅注册之间漏掉的更新。
    align();
    return unsubscribe;
  }, [sessionId]);

  return snapshot.id === sessionId ? snapshot.usage : null;
}

function formatTokenAxis(tokens: number): string {
  if (!Number.isFinite(tokens) || tokens <= 0) return "0";
  if (tokens >= 1_000_000) {
    const value = tokens / 1_000_000;
    return `${value >= 10 ? value.toFixed(0) : value.toFixed(1)}M`;
  }
  if (tokens >= 1_000) {
    const value = tokens / 1_000;
    return `${value >= 100 ? value.toFixed(0) : value.toFixed(1)}K`;
  }
  return String(Math.round(tokens));
}

/** 上下文占用项：占用率迷你条 + 百分比；超 70% 转警示色、超 90% 转危险色
 * （占用走高时整形层会把历史折叠为滚动摘要，tooltip 说明这一行为）。 */
function ContextUsageStat({ usage }: { usage: ContextUsageSnapshot }) {
  if (usage.capacityTokens <= 0 || usage.usedTokens <= 0) return null;
  const ratio = usage.usedTokens / usage.capacityTokens;
  const percent = Math.max(1, Math.round(ratio * 100));
  const level = ratio >= 0.9 ? "danger" : ratio >= 0.7 ? "warning" : "normal";
  return (
    <span
      className={cn("ai-composer-stat", level !== "normal" && `is-${level}`)}
      role="status"
      title={`最近一次请求的上下文占用：${formatTokenAxis(usage.usedTokens)} / ${formatTokenAxis(usage.capacityTokens)} tokens（${percent}%）。占用走高时，较早的对话历史会被折叠为滚动摘要以控制上下文长度。`}
    >
      <Gauge size={11} strokeWidth={2} aria-hidden="true" />
      上下文
      <span className="ai-composer-stat-bar" aria-hidden="true">
        <span
          className="ai-composer-stat-bar-fill"
          style={{ width: `${Math.min(100, percent)}%` }}
        />
      </span>
      <span className="ai-composer-stat-value">{percent}%</span>
    </span>
  );
}

/**
 * 输入框下方的会话统计条：token 速度（t/s）+ 累计输入/输出 + 上下文占用。
 * 速度优先取当前 run 的实时快照（runUsageUpdated 每次模型调用完成刷新），
 * 非运行态回退到最后一轮已完成 run 的用量。无任何数据时不占位渲染。
 */
export function SessionStatsBar({ sessionId, stats }: SessionStatsBarProps) {
  const liveRunUsage = useLiveRunUsage(sessionId);
  const runStats = liveRunUsage ?? stats.lastRunStats;
  const { inputTokens, outputTokens, contextUsage } = stats;

  const items: ReactNode[] = [];
  if (runStats) {
    items.push(
      <span
        key="speed"
        className="ai-composer-stat"
        title={`最近一轮输出 ${formatTokenCount(runStats.completionTokens)} tokens，耗时 ${(runStats.elapsedMs / 1000).toFixed(1)}s`}
      >
        <Zap size={11} strokeWidth={2} aria-hidden="true" />
        <span className="ai-composer-stat-value">
          {formatTokenGenerationSpeed(runStats.completionTokens, runStats.elapsedMs)}
        </span>
        t/s
      </span>,
    );
  }
  if (inputTokens > 0 || outputTokens > 0) {
    items.push(
      <span
        key="input"
        className="ai-composer-stat"
        title={`累计输入 ${inputTokens.toLocaleString()} tokens`}
      >
        <ArrowUp size={11} strokeWidth={2} aria-hidden="true" />
        输入
        <span className="ai-composer-stat-value">{formatTokenCount(inputTokens)}</span>
      </span>,
      <span
        key="output"
        className="ai-composer-stat"
        title={`累计输出 ${outputTokens.toLocaleString()} tokens`}
      >
        <ArrowDown size={11} strokeWidth={2} aria-hidden="true" />
        输出
        <span className="ai-composer-stat-value">{formatTokenCount(outputTokens)}</span>
      </span>,
    );
  }
  if (contextUsage && contextUsage.capacityTokens > 0 && contextUsage.usedTokens > 0) {
    items.push(<ContextUsageStat key="context" usage={contextUsage} />);
  }
  if (items.length === 0) return null;

  return (
    <div className="ai-composer-stats">
      {items.flatMap((item, index) =>
        index === 0
          ? [item]
          : [
              <span
                key={`divider-${index}`}
                className="ai-composer-stat-divider"
                aria-hidden="true"
              />,
              item,
            ],
      )}
    </div>
  );
}

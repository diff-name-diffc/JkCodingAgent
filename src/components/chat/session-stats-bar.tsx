import { useEffect, useState } from "react";
import type { DispatcherMessageUsageStats } from "../../types";
import {
  getDispatcherLiveSessionState,
  subscribeDispatcherLiveSession,
} from "../dispatcherSessionStore";
import {
  formatTokenCount,
  formatTokenGenerationSpeed,
} from "../dispatcher-chat/dispatcherChatUtils";

/** 会话统计条的静态数据（页面层按会话 memo）：累计输入/输出与最近一轮用量。 */
export interface SessionStatsData {
  /** 会话累计输入 tokens（primary 口径，跨模型求和）。 */
  inputTokens: number;
  /** 会话累计输出 tokens（primary 口径，跨模型求和）。 */
  outputTokens: number;
  /** 最近一次已完成 run 的用量快照（来自 assistant 消息 usageStats），
   * live 快照缺位（未在运行 / run 已收尾对账）时作为速度回退。 */
  lastRunStats: DispatcherMessageUsageStats | null;
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

/**
 * 输入框下方的会话统计条：token 速度（t/s）+ 累计输入/输出 tokens（k/m 缩写）。
 * 速度优先取当前 run 的实时快照（runUsageUpdated 每次模型调用完成刷新），
 * 非运行态回退到最后一轮已完成 run 的用量。无任何数据时不占位渲染。
 */
export function SessionStatsBar({ sessionId, stats }: SessionStatsBarProps) {
  const liveRunUsage = useLiveRunUsage(sessionId);
  const runStats = liveRunUsage ?? stats.lastRunStats;
  const { inputTokens, outputTokens } = stats;
  if (inputTokens <= 0 && outputTokens <= 0 && !runStats) return null;

  return (
    <div
      className="flex min-w-0 shrink items-center gap-1.5 whitespace-nowrap"
      title={`累计输入 ${inputTokens.toLocaleString()} tokens · 累计输出 ${outputTokens.toLocaleString()} tokens`}
    >
      {runStats && (
        <>
          <span>
            {formatTokenGenerationSpeed(runStats.completionTokens, runStats.elapsedMs)} t/s
          </span>
          <span aria-hidden="true">·</span>
        </>
      )}
      <span>输入 {formatTokenCount(inputTokens)}</span>
      <span aria-hidden="true">·</span>
      <span>输出 {formatTokenCount(outputTokens)}</span>
    </div>
  );
}

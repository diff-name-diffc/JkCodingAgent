import { useEffect, useMemo } from "react";

export interface RequestGuard {
  /** 领取新请求代号，并使先前所有代号失效。 */
  begin(): number;
  /** 该代号是否已过期（非最新代号）。 */
  isStale(requestId: number): boolean;
  /** 外部事件（会话切换等）直接作废当前代号，不产生新请求。 */
  invalidate(): void;
}

/**
 * 跨会话异步请求竞态防护工厂（UI-24a-1，纯逻辑）：
 * begin() 领取自增代号，isStale(id) 判定该代号是否已被后续请求/会话切换
 * 作废。工厂实例身份恒定，供 hook useMemo 持有——消费方 useCallback(deps 含
 * guard) 不再每渲染获得新身份，MessageItem 的 React.memo 在流式期间保持有效。
 */
export function createRequestGuard(): RequestGuard {
  // 从 1 起：0 是「未领取代号」的哨兵值，任何真实代号都不等于它，
  // 使未 begin 的调用方（id=0）恒判过期，而非误认为有效请求。
  let current = 1;
  return {
    begin() {
      current += 1;
      return current;
    },
    isStale(requestId) {
      return requestId !== current;
    },
    invalidate() {
      current += 1;
    },
  };
}

/**
 * 跨会话异步请求竞态防护（UI-09，范式抽自 chat-shell traceRequestRef）：
 * 会话切换使进行中的详情请求失效，避免 A 会话响应写入 B 会话界面。
 *
 * UI-24a-1：返回的 guard 实例身份跨渲染恒定（useMemo + 工厂）——此前每次
 * 渲染返回新对象字面量，消费方 useCallback(deps 含 guard) 连带每渲染换身份，
 * 击穿 MessageItem 的 React.memo（流式期间每个可见行每 token 全量 reconcile）。
 */
export function useSessionRequestGuard(sessionId: string | null): RequestGuard {
  const guard = useMemo(() => createRequestGuard(), []);

  useEffect(() => {
    guard.invalidate();
  }, [sessionId, guard]);

  return guard;
}

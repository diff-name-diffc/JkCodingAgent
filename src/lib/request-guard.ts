/**
 * request-guard.ts — 跨会话异步请求竞态防护工厂（UI-24a-1）。
 *
 * 从 useSessionRequestGuard 抽出的纯逻辑：begin() 领取自增代号，
 * isStale(id) 判定该代号是否已被后续请求/会话切换作废。工厂实例身份
 * 恒定，供 hook useMemo 持有——消费方 useCallback(deps 含 guard) 不再
 * 每渲染获得新身份，MessageItem 的 React.memo 在流式期间保持有效。
 */

export interface RequestGuard {
  /** 领取新请求代号，并使先前所有代号失效。 */
  begin(): number;
  /** 该代号是否已过期（非最新代号）。 */
  isStale(requestId: number): boolean;
  /** 外部事件（会话切换等）直接作废当前代号，不产生新请求。 */
  invalidate(): void;
}

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

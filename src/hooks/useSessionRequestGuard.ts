import { useEffect, useMemo } from "react";
import { createRequestGuard, type RequestGuard } from "../lib/request-guard";

/**
 * 跨会话异步请求竞态防护（UI-09，范式抽自 chat-shell traceRequestRef）：
 * 会话切换使进行中的详情请求失效，避免 A 会话响应写入 B 会话界面。
 *
 * UI-24a-1：返回的 guard 实例身份跨渲染恒定（useMemo + lib/request-guard
 * 工厂）——此前每次渲染返回新对象字面量，消费方 useCallback(deps 含 guard)
 * 连带每渲染换身份，击穿 MessageItem 的 React.memo（流式期间每个可见行
 * 每 token 全量 reconcile）。
 */
export function useSessionRequestGuard(sessionId: string | null): RequestGuard {
  const guard = useMemo(() => createRequestGuard(), []);

  useEffect(() => {
    guard.invalidate();
  }, [sessionId, guard]);

  return guard;
}

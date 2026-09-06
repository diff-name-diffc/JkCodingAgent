import { useCallback, useEffect, useRef } from "react";

/**
 * 跨会话异步请求竞态防护（UI-09，范式抽自 chat-shell traceRequestRef）：
 * 会话切换使进行中的详情请求失效，避免 A 会话响应写入 B 会话界面。
 */
export function useSessionRequestGuard(sessionId: string | null) {
  const ref = useRef(0);

  useEffect(() => {
    ref.current += 1;
  }, [sessionId]);

  const begin = useCallback(() => {
    ref.current += 1;
    return ref.current;
  }, []);

  const isStale = useCallback((requestId: number) => requestId !== ref.current, []);

  return { begin, isStale };
}

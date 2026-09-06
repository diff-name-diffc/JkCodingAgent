import { createContext, useContext } from "react";

/**
 * 当前聊天面 sessionId 的作用域上下文（UI-08）：深层组件（如消息内的
 * GraphPlanCard）无需逐层透传即可获得归属会话，用于把临时详情状态
 * 绑定到正确会话，避免跨会话串台。
 */
export const SessionScopeContext = createContext<string | null>(null);

export function useSessionScope(): string | null {
  return useContext(SessionScopeContext);
}

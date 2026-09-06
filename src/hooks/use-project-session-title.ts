import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { DispatcherSession } from "../types";
import { flattenSessionPages, useProjectSessionsQuery } from "./use-session-queries";

/**
 * 项目会话标题（UI-11 头部任务名）。
 *
 * 数据源两层：
 * 1. 与 SessionPanel 的 `useProjectSessionsQuery` 共享 queryKey —— React Query
 *    去重，SessionPanel 挂载时零额外请求；其事件合并（useSessionListEventMerge）
 *    会持续刷新缓存，本 hook 直接受益。
 * 2. 自监听 `dispatcher-session-updated` 兜底 —— 导航收起 / SessionPanel 卸载时
 *    事件合并 hook 不在，缓存可能过期，事件里的 title 优先。
 *
 * 切换会话时清空事件标题，避免旧会话标题闪现。
 */
export function useProjectSessionTitle(
  projectId: string | null,
  sessionId: string | null,
): string | null {
  const sessionsQuery = useProjectSessionsQuery(projectId ?? "", Boolean(projectId));
  const [eventTitle, setEventTitle] = useState<string | null>(null);

  useEffect(() => {
    setEventTitle(null);
  }, [sessionId]);

  useEffect(() => {
    if (!projectId || !sessionId) return;
    const unlisten = listen<DispatcherSession>("dispatcher-session-updated", (event) => {
      const payload = event.payload;
      if (payload.kind !== "project" || payload.projectId !== projectId) return;
      if (payload.id !== sessionId) return;
      setEventTitle(payload.title?.trim() ? payload.title : null);
    });
    return () => {
      unlisten.then((stopListening) => stopListening()).catch(() => {});
    };
  }, [projectId, sessionId]);

  const cachedSession = flattenSessionPages(sessionsQuery.data).find(
    (session) => session.id === sessionId,
  );
  const cachedTitle = cachedSession?.title?.trim() ? cachedSession.title : null;
  return eventTitle ?? cachedTitle;
}

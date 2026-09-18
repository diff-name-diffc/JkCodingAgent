import { confirm } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useMemo, useState } from "react";
import {
  useChatCategoriesQuery,
  useCreateChatCategory,
  useCreateChatSession,
  useDeleteChatCategory,
  useDeleteChatSession,
  useSetChatSessionCategory,
  useChatSessionUpdates,
  useChatSessionsQuery,
  useUpdateChatCategory,
} from "../../hooks/use-chat-queries";
import { useSessionSearchQuery } from "../../hooks/use-session-queries";
import {
  cleanupDispatcherSession,
  getDispatcherSessionRunning,
} from "../dispatcherSessionStore";
import { useToast } from "../Toast";
import { resolveActiveChatCategory } from "./active-chat-category";

interface UseChatSessionControllerOptions {
  activeSessionId: string | null;
  isPlainChat: boolean;
  embedded: boolean;
  setActiveSessionId: (sessionId: string | null) => void;
  resetConversation: () => void;
  onSessionChange?: (sessionId: string | null) => void;
}

export function useChatSessionController({
  activeSessionId,
  isPlainChat,
  embedded,
  setActiveSessionId,
  resetConversation,
  onSessionChange,
}: UseChatSessionControllerOptions) {
  const enabled = isPlainChat && !embedded;
  const { showToast } = useToast();
  const [search, setSearch] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");
  useChatSessionUpdates(enabled);
  const sessionsQuery = useChatSessionsQuery(undefined, enabled);
  const categoriesQuery = useChatCategoriesQuery(enabled);
  const createCategory = useCreateChatCategory();
  const { mutateAsync: createSession } = useCreateChatSession();
  const updateCategory = useUpdateChatCategory();
  const deleteCategory = useDeleteChatCategory();
  const { mutateAsync: deleteSession } = useDeleteChatSession();
  const setSessionCategory = useSetChatSessionCategory();
  const sessionSearchQuery = useSessionSearchQuery({
    query: debouncedSearch,
    kind: "chat",
    enabled,
  });
  useEffect(() => {
    const timer = window.setTimeout(() => setDebouncedSearch(search), 260);
    return () => window.clearTimeout(timer);
  }, [search]);

  const selectSession = useCallback(
    (sessionId: string | null) => {
      setActiveSessionId(sessionId);
      resetConversation();
    },
    [resetConversation, setActiveSessionId],
  );

  const newConversation = useCallback(() => {
    if (!embedded) setActiveSessionId(null);
    resetConversation();
  }, [embedded, resetConversation, setActiveSessionId]);

  const newSessionInCategory = useCallback(
    async (categoryId: string) => {
      if (!enabled) return;
      try {
        const session = await createSession({ title: "新对话", category: categoryId });
        setActiveSessionId(session.id);
        resetConversation();
      } catch (error) {
        console.error("在分类下创建会话失败:", error);
      }
    },
    [createSession, enabled, resetConversation, setActiveSessionId],
  );

  const deleteChatSession = useCallback(
    async (sessionId: string) => {
      if (!enabled) return;
      // 运行中的会话前置拦截（后端 session_delete 同口径 fail-closed）：
      // 先给可读提示，避免用户走完确认对话框才收到命令层报错。
      if (getDispatcherSessionRunning(sessionId)) {
        showToast("该会话正在运行中，请先停止生成后再删除。", "warning");
        return;
      }
      const confirmed = await confirm("确定永久删除这个会话吗？相关消息和文件也会一并删除。", {
        title: "删除会话",
        kind: "warning",
      });
      if (!confirmed) return;
      try {
        await deleteSession(sessionId);
        cleanupDispatcherSession(sessionId);
        if (sessionId === activeSessionId) {
          const next = (sessionsQuery.data ?? []).find((session) => session.id !== sessionId);
          setActiveSessionId(next?.id ?? null);
          resetConversation();
        }
      } catch (error) {
        console.error("删除聊天会话失败:", error);
        showToast(`删除会话失败：${String(error)}`, "error");
      }
    },
    [
      activeSessionId,
      deleteSession,
      enabled,
      resetConversation,
      sessionsQuery.data,
      setActiveSessionId,
      showToast,
    ],
  );

  const createChatCategory = useCallback(
    (name: string, config?: { systemPrompt?: string; allowedTools?: string[] }) => {
      if (!enabled) return;
      createCategory.mutate({ name, ...config });
    },
    [createCategory, enabled],
  );
  const renameChatCategory = useCallback(
    (categoryId: string, name: string) => {
      if (enabled) updateCategory.mutate({ categoryId, name });
    },
    [enabled, updateCategory],
  );
  const deleteChatCategory = useCallback(
    (categoryId: string) => {
      if (enabled) deleteCategory.mutate(categoryId);
    },
    [deleteCategory, enabled],
  );
  const moveSession = useCallback(
    (workspaceId: string, categoryId: string) => {
      if (!enabled) return;
      setSessionCategory.mutate({ workspaceId, categoryId });
      if (workspaceId === activeSessionId) onSessionChange?.(workspaceId);
    },
    [activeSessionId, enabled, onSessionChange, setSessionCategory],
  );

  const trimmedSearch = debouncedSearch.trim();
  // 当前会话所属分类（头部徽标 / 分类化空态共用）。用未过搜索过滤的
  // 列表解析，与 activeTitle 同源；分类记录缺失时为 null。
  const activeCategory = useMemo(
    () =>
      resolveActiveChatCategory(
        sessionsQuery.data ?? [],
        categoriesQuery.data ?? [],
        activeSessionId,
      ),
    [activeSessionId, categoriesQuery.data, sessionsQuery.data],
  );
  // 会话搜索/列表错误显式重试（UI-25 登记遗留）：refetch 身份由 React Query 保证稳定。
  const refetchSessions = sessionsQuery.refetch;
  const refetchSearch = sessionSearchQuery.refetch;
  const retrySessions = useCallback(() => {
    void (trimmedSearch ? refetchSearch() : refetchSessions());
  }, [refetchSearch, refetchSessions, trimmedSearch]);
  const sessions = useMemo(
    () =>
      trimmedSearch
        ? (sessionSearchQuery.data ?? []).map((result) => ({
            id: result.sessionId,
            title: result.sessionTitle,
            category: result.category,
            createdAt: result.updatedAt,
            updatedAt: result.updatedAt,
            keywords: result.keywords,
          }))
        : (sessionsQuery.data ?? []),
    [sessionSearchQuery.data, sessionsQuery.data, trimmedSearch],
  );

  return {
    search,
    setSearch,
    sessions,
    categories: categoriesQuery.data ?? [],
    categoriesLoading: categoriesQuery.isLoading,
    activeCategory,
    sessionsLoading: trimmedSearch
      ? sessionSearchQuery.isLoading
      : sessionsQuery.isLoading || categoriesQuery.isLoading,
    sessionsError:
      trimmedSearch && sessionSearchQuery.error ? String(sessionSearchQuery.error) : undefined,
    retrySessions,
    searchActive: Boolean(trimmedSearch),
    activeTitle:
      (sessionsQuery.data ?? []).find((session) => session.id === activeSessionId)?.title ?? null,
    selectSession,
    newConversation,
    newSessionInCategory,
    deleteChatSession,
    createChatCategory,
    renameChatCategory,
    deleteChatCategory,
    moveSession,
  };
}

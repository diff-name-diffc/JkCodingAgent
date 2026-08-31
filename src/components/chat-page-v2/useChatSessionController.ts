import { confirm } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
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
import { cleanupDispatcherSession } from "../dispatcherSessionStore";

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
  const pendingSessionRef = useRef<Promise<string | null> | null>(null);

  useEffect(() => {
    const timer = window.setTimeout(() => setDebouncedSearch(search), 260);
    return () => window.clearTimeout(timer);
  }, [search]);

  const ensureSession = useCallback(async (): Promise<string | null> => {
    if (activeSessionId) return activeSessionId;
    if (!isPlainChat) return null;
    if (pendingSessionRef.current) return pendingSessionRef.current;
    const pending = createSession({ title: "新对话", category: "tech" })
      .then((session) => {
        setActiveSessionId(session.id);
        return session.id;
      })
      .catch((error) => {
        console.error("创建聊天会话失败:", error);
        return null;
      })
      .finally(() => {
        pendingSessionRef.current = null;
      });
    pendingSessionRef.current = pending;
    return pending;
  }, [activeSessionId, createSession, isPlainChat, setActiveSessionId]);

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
      }
    },
    [
      activeSessionId,
      deleteSession,
      enabled,
      resetConversation,
      sessionsQuery.data,
      setActiveSessionId,
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
    sessionsLoading: trimmedSearch
      ? sessionSearchQuery.isLoading
      : sessionsQuery.isLoading || categoriesQuery.isLoading,
    sessionsError:
      trimmedSearch && sessionSearchQuery.error ? String(sessionSearchQuery.error) : undefined,
    searchActive: Boolean(trimmedSearch),
    activeTitle:
      (sessionsQuery.data ?? []).find((session) => session.id === activeSessionId)?.title ?? null,
    ensureSession,
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

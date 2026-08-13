import { useEffect } from "react";
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AhaSettingsV2,
  ChatCategory,
  ChatSession,
  DispatcherSession,
  ModelLibraryEntry,
  SessionKeyword,
  SessionSearchResult,
  SessionPage,
} from "../types";
import {
  withDispatcherSessionRunning,
  withDispatcherSessionsRunning,
} from "../components/dispatcherSessionStore";
import { bindPurpose } from "../components/settings/providers/provider-registry";
import {
  hasAnyPurposeConfigs,
  seedModelLibrary,
} from "../components/settings/providers/model-library";

/**
 * TanStack Query hooks for the Chat UI.
 *
 * Request/response data is backed by typed Tauri commands. Streaming
 * continues to flow through the dispatcherSessionStore singleton + Tauri
 * Channel pipeline; these hooks only cover the request/response surface
 * (lists, single conversation, mutations).
 */

const QUERY_KEYS = {
  sessions: (category?: string) => ["chat", "sessions", category ?? "all"] as const,
  categorySessions: (category: string) => ["chat", "sessions", "category", category] as const,
  sessionSearch: (query: string, kind: string, projectId?: string | null) =>
    ["sessions", "search", kind, projectId ?? "all", query] as const,
  categories: ["chat", "categories"] as const,
  messages: (sessionId: string) => ["chat", "messages", sessionId] as const,
  models: ["chat", "models"] as const,
} as const;

// ── Sessions ──────────────────────────────────────────────────────────────

function sortChatSessionsByUpdatedAt(sessions: ChatSession[]) {
  return [...sessions].sort((left, right) => {
    const leftTime = Date.parse(left.updatedAt);
    const rightTime = Date.parse(right.updatedAt);
    return (Number.isNaN(rightTime) ? 0 : rightTime) - (Number.isNaN(leftTime) ? 0 : leftTime);
  });
}

export function useChatSessionUpdates(enabled = true) {
  const qc = useQueryClient();

  useEffect(() => {
    if (!enabled) return;

    const unlistenSession = listen<DispatcherSession>("dispatcher-session-updated", (event) => {
      const payload = event.payload;
      if (payload.kind !== "chat") return;

      const updatedSession = withDispatcherSessionRunning<ChatSession>({
        id: payload.id,
        title: payload.title,
        category: payload.category,
        createdAt: payload.createdAt,
        updatedAt: payload.updatedAt,
        keywords: payload.keywords ?? [],
      });

      qc.setQueryData<ChatSession[]>(QUERY_KEYS.sessions(), (sessions) => {
        if (!sessions) return sessions;
        const exists = sessions.some((session) => session.id === updatedSession.id);
        const next = exists
          ? sessions.map((session) =>
              session.id === updatedSession.id
                ? {
                    ...updatedSession,
                    keywords: payload.keywords ?? session.keywords,
                  }
                : session,
            )
          : [updatedSession, ...sessions];
        return sortChatSessionsByUpdatedAt(next);
      });
      void qc.invalidateQueries({ queryKey: ["chat", "sessions"] });
      void qc.invalidateQueries({ queryKey: ["sessions", "search"] });
    });
    const unlistenKeywords = listen<{
      sessionId: string;
      keywords: SessionKeyword[];
    }>("session-keywords-updated", (event) => {
      const { sessionId, keywords } = event.payload;
      const values = keywords.map((keyword) => keyword.keyword);
      qc.setQueryData<ChatSession[]>(QUERY_KEYS.sessions(), (sessions) =>
        sessions?.map((session) =>
          session.id === sessionId ? { ...session, keywords: values } : session,
        ),
      );
      void qc.invalidateQueries({ queryKey: ["chat", "sessions"] });
      void qc.invalidateQueries({ queryKey: ["sessions", "search"] });
    });

    return () => {
      unlistenSession.then((stopListening) => stopListening()).catch(() => {});
      unlistenKeywords.then((stopListening) => stopListening()).catch(() => {});
    };
  }, [enabled, qc]);
}

export function useChatSessionsQuery(category?: string, enabled = true) {
  return useQuery({
    queryKey: QUERY_KEYS.sessions(category),
    queryFn: async () => {
      const page = await invoke<SessionPage<ChatSession>>("chat_list_sessions", {
        category: category ?? null,
        cursor: null,
        pageSize: 100,
      });
      return withDispatcherSessionsRunning(page.items);
    },
    enabled,
  });
}

const CHAT_CATEGORY_PAGE_SIZE = 20;

export function useChatCategorySessionsQuery(category: string, enabled = true) {
  return useInfiniteQuery({
    queryKey: QUERY_KEYS.categorySessions(category),
    queryFn: async ({ pageParam }) => {
      const page = await invoke<SessionPage<ChatSession>>("chat_list_sessions", {
        category,
        cursor: pageParam,
        pageSize: CHAT_CATEGORY_PAGE_SIZE,
      });
      if (page.hasMore && !page.nextCursor) {
        throw new Error(`分类 ${category} 的分页响应缺少 nextCursor`);
      }
      return {
        ...page,
        items: withDispatcherSessionsRunning(page.items),
      };
    },
    initialPageParam: null as string | null,
    getNextPageParam: (lastPage) => lastPage.hasMore ? lastPage.nextCursor : undefined,
    enabled,
  });
}

export function useCreateChatSession() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (args: { title: string; category?: string }) =>
      invoke<ChatSession>("chat_create_session", {
        title: args.title,
        category: args.category ?? "tech",
      }),
    onSuccess: (createdSession) => {
      qc.setQueryData<ChatSession[]>(QUERY_KEYS.sessions(), (sessions = []) => [
        createdSession,
        ...sessions.filter((session) => session.id !== createdSession.id),
      ]);
      void qc.invalidateQueries({ queryKey: ["chat", "sessions"] });
      void qc.invalidateQueries({ queryKey: QUERY_KEYS.categories });
    },
  });
}

export function useDeleteChatSession() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (sessionId: string) =>
      invoke<void>("chat_delete_session", { sessionId }),
    onSuccess: (_data, sessionId) => {
      qc.setQueryData<ChatSession[]>(QUERY_KEYS.sessions(), (sessions) =>
        sessions?.filter((session) => session.id !== sessionId),
      );
      qc.setQueriesData<SessionSearchResult[]>(
        { queryKey: ["sessions", "search"] },
        (results) => results?.filter((result) => result.sessionId !== sessionId),
      );
      qc.removeQueries({ queryKey: QUERY_KEYS.messages(sessionId) });
      void qc.invalidateQueries({ queryKey: ["chat", "sessions"] });
      void qc.invalidateQueries({ queryKey: QUERY_KEYS.categories });
    },
  });
}

export function useSessionSearchQuery({
  query,
  kind,
  projectId = null,
  limit = 20,
  enabled = true,
}: {
  query: string;
  kind: "chat" | "project";
  projectId?: string | null;
  limit?: number;
  enabled?: boolean;
}) {
  const trimmed = query.trim();
  return useQuery({
    queryKey: QUERY_KEYS.sessionSearch(trimmed, kind, projectId),
    queryFn: () =>
      invoke<SessionSearchResult[]>("session_search_keywords", {
        query: trimmed,
        limit,
        kind,
        projectId,
      }),
    enabled: enabled && trimmed.length > 0,
  });
}

// ── Categories ────────────────────────────────────────────────────────────

export function useChatCategoriesQuery(enabled = true) {
  return useQuery({
    queryKey: QUERY_KEYS.categories,
    queryFn: () => invoke<ChatCategory[]>("chat_list_categories"),
    enabled,
  });
}

export function useCreateChatCategory() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (args: {
      name: string;
      icon?: string;
      color?: string;
      allowedTools?: string[];
      systemPrompt?: string;
    }) =>
      invoke<ChatCategory>("chat_create_category", {
        name: args.name,
        icon: args.icon ?? "Folder",
        color: args.color ?? "#297c70",
        allowedTools: args.allowedTools ?? null,
        systemPrompt: args.systemPrompt ?? null,
      }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: QUERY_KEYS.categories });
    },
  });
}

export function useUpdateChatCategory() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (args: {
      categoryId: string;
      name?: string;
      icon?: string;
      color?: string;
    }) =>
      invoke<ChatCategory | null>("chat_update_category", {
        categoryId: args.categoryId,
        name: args.name ?? null,
        icon: args.icon ?? null,
        color: args.color ?? null,
      }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: QUERY_KEYS.categories });
      void qc.invalidateQueries({ queryKey: ["chat", "sessions"] });
    },
  });
}

export function useDeleteChatCategory() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (categoryId: string) =>
      invoke<void>("chat_delete_category", { categoryId }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: QUERY_KEYS.categories });
      void qc.invalidateQueries({ queryKey: ["chat", "sessions"] });
    },
  });
}

export function useSetChatSessionCategory() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (args: { workspaceId: string; categoryId: string }) =>
      invoke<void>("chat_set_session_category_v6", {
        sessionId: args.workspaceId,
        categoryId: args.categoryId,
      }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: QUERY_KEYS.categories });
      void qc.invalidateQueries({ queryKey: ["chat", "sessions"] });
    },
  });
}

// ── Models ────────────────────────────────────────────────────────────────

export function useChatModelsQuery() {
  return useQuery({
    queryKey: QUERY_KEYS.models,
    queryFn: async () => {
      // 聊天输入框的可选模型与设置页「聊天主模型」共用统一数据源：分类模型库
      // （AhaSettingsV2.modelLibrary 的 text 分类）。这里返回整份设置，由组件
      // 用 entriesForCategory 取选项、用 getPurposeBinding 取当前生效绑定。
      const loaded = await invoke<AhaSettingsV2>("aha_get_settings_v2");
      // 与设置页一致：旧用户已有用途配置但模型库为空时，按分类播种（仅内存，
      // 选择落盘由 useBindChatModel / 设置页负责），避免下拉为空。
      const needsSeed =
        (loaded.modelLibrary ?? []).length === 0 && hasAnyPurposeConfigs(loaded);
      return needsSeed ? { ...loaded, modelLibrary: seedModelLibrary(loaded) } : loaded;
    },
  });
}

export function useBindChatModel() {
  const qc = useQueryClient();
  return useMutation({
    // 选中模型库条目即把它绑定为「聊天主模型」用途（写入 chat.chatModelConfigs，
    // 运行时与设置页消费的结构不变），与设置页 PurposeSelect 走同一 bindPurpose。
    mutationFn: async (entry: ModelLibraryEntry) => {
      const current = await invoke<AhaSettingsV2>("aha_get_settings_v2");
      const next = bindPurpose(current, "chatChat", entry);
      return invoke<AhaSettingsV2>("aha_save_settings_v2", { settings: next });
    },
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: QUERY_KEYS.models });
    },
  });
}

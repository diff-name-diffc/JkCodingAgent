import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import type {
  ChatCategory,
  ChatSession,
  DispatcherMessage,
  DispatcherModelConfig,
  SessionSearchResult,
  SessionPage,
} from "../types";
import { withDispatcherSessionsRunning } from "../components/dispatcherSessionStore";
import { normalizeDispatcherMessage } from "../components/dispatcher-chat/dispatcherChatUtils";

/**
 * TanStack Query hooks for the Chat UI.
 *
 * All backed by existing Tauri commands — no backend changes. Streaming
 * continues to flow through the dispatcherSessionStore singleton + Tauri
 * Channel pipeline; these hooks only cover the request/response surface
 * (lists, single conversation, mutations).
 */

const QUERY_KEYS = {
  sessions: (category?: string) => ["chat", "sessions", category ?? "all"] as const,
  sessionSearch: (query: string, kind: string, projectId?: string | null) =>
    ["sessions", "search", kind, projectId ?? "all", query] as const,
  categories: ["chat", "categories"] as const,
  messages: (sessionId: string) => ["chat", "messages", sessionId] as const,
  models: ["chat", "models"] as const,
} as const;

// ── Sessions ──────────────────────────────────────────────────────────────

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

export function useCreateChatSession() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (args: { title: string; category?: string }) =>
      invoke<ChatSession>("chat_create_session", {
        title: args.title,
        category: args.category ?? "tech",
      }),
    onSuccess: () => {
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
    onSuccess: () => {
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
        color: args.color ?? "#21f4df",
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
      invoke<void>("chat_set_session_category", {
        workspaceId: args.workspaceId,
        categoryId: args.categoryId,
      }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: QUERY_KEYS.categories });
      void qc.invalidateQueries({ queryKey: ["chat", "sessions"] });
    },
  });
}

// ── Messages ──────────────────────────────────────────────────────────────

export function useConversationMessagesQuery(
  workspaceId: string | null,
  enabled = true,
) {
  return useQuery({
    queryKey: workspaceId ? QUERY_KEYS.messages(workspaceId) : ["chat", "messages", "none"],
    queryFn: () =>
      invoke<DispatcherMessage[]>("dispatcher_list_messages", { workspaceId }).then(
        (rows) => rows.map(normalizeDispatcherMessage),
      ),
    enabled: Boolean(workspaceId) && enabled,
  });
}

export function useClearMessages() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (workspaceId: string) =>
      invoke<void>("dispatcher_clear_messages", { workspaceId }),
    onSuccess: (_data, workspaceId) => {
      void qc.invalidateQueries({ queryKey: QUERY_KEYS.messages(workspaceId) });
    },
  });
}

// ── Models ────────────────────────────────────────────────────────────────

export function useChatModelsQuery() {
  return useQuery({
    queryKey: QUERY_KEYS.models,
    queryFn: async () => {
      // Reuse the existing settings v2 command — the chat model configs live
      // there. Components that need only active models can filter client-side.
      const settings = await invoke<{
        chat: { chatModelConfigs: DispatcherModelConfig[] };
      }>("aha_get_settings_v2");
      return settings.chat.chatModelConfigs;
    },
  });
}

export function useSetActiveChatModel() {
  const qc = useQueryClient();
  return useMutation({
    // Backend takes a model index, not a model name — see
    // src-tauri/src/agent/commands.rs:aha_set_active_chat_model.
    mutationFn: (modelIndex: number) =>
      invoke<DispatcherModelConfig[]>("aha_set_active_chat_model", {
        modelIndex,
      }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: QUERY_KEYS.models });
    },
  });
}

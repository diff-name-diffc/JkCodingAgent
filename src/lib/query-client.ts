import { QueryClient } from "@tanstack/react-query";

/**
 * Shared TanStack Query client for the refactored Chat UI.
 *
 * Used for: chat session list, single-conversation messages, model list,
 * categories. Streaming is NOT routed through Query — it continues to use the
 * existing Tauri Channel + dispatcherSessionStore pipeline, which already
 * handles 50 tokens/s with rAF batching.
 */
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // The data is local (Tauri SQLite); network-style refetch-on-focus
      // would cause redundant invokes. Keep it explicit and manual.
      refetchOnWindowFocus: false,
      staleTime: 30_000,
      retry: 1,
    },
    mutations: {
      retry: 0,
    },
  },
});

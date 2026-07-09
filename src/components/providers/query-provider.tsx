import * as React from "react";
import { QueryClientProvider } from "@tanstack/react-query";
import { queryClient } from "../../lib/query-client";

/**
 * Wraps the app with the shared TanStack Query client.
 * Mounted once, high in the tree (see main.tsx / App.tsx).
 */
export function QueryProvider({ children }: { children: React.ReactNode }) {
  return (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
}

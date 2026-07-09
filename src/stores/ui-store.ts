import { create } from "zustand";
import { persist } from "zustand/middleware";
import type { ThemeMode } from "../types";

/**
 * Pure-frontend UI state for the refactored Chat surface.
 *
 * Scope deliberately kept narrow:
 *   - layout flags (sidebar / artifact panel)
 *   - theme mirror (the source of truth for applying `html.dark` still lives
 *     in App.tsx so we don't break the existing Radix Themes Theme wrapper;
 *     this store only mirrors the user's *choice* for components that need to
 *     read it without prop-drilling)
 *   - active conversation id
 *   - command-palette open state
 *
 * Chat message state, streaming state and the existing singleton stores
 * (dispatcherSessionStore / subAgentEventStore) are NOT moved here — they are
 * kept as-is per the refactor's "do not break existing logic" constraint.
 */
export interface UIState {
  sidebarCollapsed: boolean;
  artifactPanelOpen: boolean;
  theme: ThemeMode;
  activeConversationId: string | null;
  commandPaletteOpen: boolean;

  toggleSidebar: () => void;
  setSidebarCollapsed: (collapsed: boolean) => void;

  setArtifactPanelOpen: (open: boolean) => void;
  toggleArtifactPanel: () => void;

  setTheme: (theme: ThemeMode) => void;

  setActiveConversationId: (id: string | null) => void;

  setCommandPaletteOpen: (open: boolean) => void;
  toggleCommandPalette: () => void;
}

export const useUIStore = create<UIState>()(
  persist(
    (set) => ({
      sidebarCollapsed: false,
      artifactPanelOpen: false,
      theme: "system",
      activeConversationId: null,
      commandPaletteOpen: false,

      toggleSidebar: () => set((s) => ({ sidebarCollapsed: !s.sidebarCollapsed })),
      setSidebarCollapsed: (collapsed) => set({ sidebarCollapsed: collapsed }),

      setArtifactPanelOpen: (open) => set({ artifactPanelOpen: open }),
      toggleArtifactPanel: () =>
        set((s) => ({ artifactPanelOpen: !s.artifactPanelOpen })),

      setTheme: (theme) => set({ theme }),

      setActiveConversationId: (id) => set({ activeConversationId: id }),

      setCommandPaletteOpen: (open) => set({ commandPaletteOpen: open }),
      toggleCommandPalette: () =>
        set((s) => ({ commandPaletteOpen: !s.commandPaletteOpen })),
    }),
    {
      name: "jkcodingagent:ui",
      partialize: (s) => ({
        sidebarCollapsed: s.sidebarCollapsed,
        theme: s.theme,
      }),
    },
  ),
);

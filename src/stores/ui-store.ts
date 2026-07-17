import { create } from "zustand";
import { persist } from "zustand/middleware";

/**
 * Pure-frontend UI state for the Chat surface.
 *
 * Scope:
 *   - layout flags (sidebar / artifact panel)
 *   - active conversation id
 *   - command-palette open state
 *
 * The app ships a single light theme, so there is no theme state here.
 */
export interface UIState {
  sidebarCollapsed: boolean;
  artifactPanelOpen: boolean;
  activeConversationId: string | null;
  commandPaletteOpen: boolean;

  toggleSidebar: () => void;
  setSidebarCollapsed: (collapsed: boolean) => void;

  setArtifactPanelOpen: (open: boolean) => void;
  toggleArtifactPanel: () => void;

  setActiveConversationId: (id: string | null) => void;

  setCommandPaletteOpen: (open: boolean) => void;
  toggleCommandPalette: () => void;
}

export const useUIStore = create<UIState>()(
  persist(
    (set) => ({
      sidebarCollapsed: false,
      artifactPanelOpen: false,
      activeConversationId: null,
      commandPaletteOpen: false,

      toggleSidebar: () => set((s) => ({ sidebarCollapsed: !s.sidebarCollapsed })),
      setSidebarCollapsed: (collapsed) => set({ sidebarCollapsed: collapsed }),

      setArtifactPanelOpen: (open) => set({ artifactPanelOpen: open }),
      toggleArtifactPanel: () =>
        set((s) => ({ artifactPanelOpen: !s.artifactPanelOpen })),

      setActiveConversationId: (id) => set({ activeConversationId: id }),

      setCommandPaletteOpen: (open) => set({ commandPaletteOpen: open }),
      toggleCommandPalette: () =>
        set((s) => ({ commandPaletteOpen: !s.commandPaletteOpen })),
    }),
    {
      name: "jkcodingagent:ui",
      partialize: (s) => ({
        sidebarCollapsed: s.sidebarCollapsed,
      }),
      // Clean up legacy theme keys left over from the removed theme switcher.
      onRehydrateStorage: () => () => {
        localStorage.removeItem("jkcodingagent:theme");
      },
    },
  ),
);

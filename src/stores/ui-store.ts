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
  /** 展开状态下的侧边栏宽度（px），可通过边框拖拽调整。 */
  sidebarWidth: number;
  artifactPanelOpen: boolean;
  activeConversationId: string | null;
  commandPaletteOpen: boolean;
  /** 当前打开的图编排面板对应的 planId（null = 关闭）。不持久化。 */
  graphPanelPlanId: string | null;

  toggleSidebar: () => void;
  setSidebarCollapsed: (collapsed: boolean) => void;
  setSidebarWidth: (width: number) => void;

  setArtifactPanelOpen: (open: boolean) => void;
  toggleArtifactPanel: () => void;

  setActiveConversationId: (id: string | null) => void;

  setCommandPaletteOpen: (open: boolean) => void;
  toggleCommandPalette: () => void;

  setGraphPanelPlanId: (planId: string | null) => void;
}

export const useUIStore = create<UIState>()(
  persist(
    (set) => ({
      sidebarCollapsed: false,
      sidebarWidth: 264,
      artifactPanelOpen: false,
      activeConversationId: null,
      commandPaletteOpen: false,
      graphPanelPlanId: null,

      toggleSidebar: () => set((s) => ({ sidebarCollapsed: !s.sidebarCollapsed })),
      setSidebarCollapsed: (collapsed) => set({ sidebarCollapsed: collapsed }),
      setSidebarWidth: (width) => set({ sidebarWidth: width }),

      setArtifactPanelOpen: (open) => set({ artifactPanelOpen: open }),
      toggleArtifactPanel: () =>
        set((s) => ({ artifactPanelOpen: !s.artifactPanelOpen })),

      setActiveConversationId: (id) => set({ activeConversationId: id }),

      setCommandPaletteOpen: (open) => set({ commandPaletteOpen: open }),
      toggleCommandPalette: () =>
        set((s) => ({ commandPaletteOpen: !s.commandPaletteOpen })),

      setGraphPanelPlanId: (planId) => set({ graphPanelPlanId: planId }),
    }),
    {
      name: "jkcodingagent:ui",
      partialize: (s) => ({
        sidebarCollapsed: s.sidebarCollapsed,
        sidebarWidth: s.sidebarWidth,
      }),
      // Clean up legacy theme keys left over from the removed theme switcher.
      onRehydrateStorage: () => () => {
        localStorage.removeItem("jkcodingagent:theme");
      },
    },
  ),
);

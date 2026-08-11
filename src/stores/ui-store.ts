import { create } from "zustand";
import { persist } from "zustand/middleware";

/**
 * Pure-frontend UI state for the Chat surface.
 *
 * Scope:
 *   - layout flags (sidebar / artifact panel)
 *   - command-palette open state
 *
 * The app ships a single light theme, so there is no theme state here.
 */
export interface UIState {
  sidebarCollapsed: boolean;
  /** 展开状态下的侧边栏宽度（px），可通过边框拖拽调整。 */
  sidebarWidth: number;
  artifactPanelOpen: boolean;
  commandPaletteOpen: boolean;
  /** 当前打开的图编排面板对应的 planId（null = 关闭）。不持久化。 */
  graphPanelPlanId: string | null;

  toggleSidebar: () => void;
  setSidebarWidth: (width: number) => void;

  setArtifactPanelOpen: (open: boolean) => void;

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
      commandPaletteOpen: false,
      graphPanelPlanId: null,

      toggleSidebar: () => set((s) => ({ sidebarCollapsed: !s.sidebarCollapsed })),
      setSidebarWidth: (width) => set({ sidebarWidth: width }),

      setArtifactPanelOpen: (open) => set({ artifactPanelOpen: open }),

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

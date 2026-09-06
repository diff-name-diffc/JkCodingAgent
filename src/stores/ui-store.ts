import { create } from "zustand";
import { persist } from "zustand/middleware";

/**
 * Pure-frontend UI state for the Chat surface.
 *
 * Scope:
 *   - layout flags (sidebar / artifact panel)
 *   - command-palette open state
 *
 * 主题偏好不在这里：权威源是后端 AhaSettingsV2.theme（lib/theme.ts 校准）。
 * 工作区布局偏好与执行图归属已迁至 workspace-store（UI-08）。
 */
export interface UIState {
  sidebarCollapsed: boolean;
  /** 展开状态下的侧边栏宽度（px），可通过边框拖拽调整。 */
  sidebarWidth: number;
  artifactPanelOpen: boolean;
  commandPaletteOpen: boolean;

  toggleSidebar: () => void;
  setSidebarWidth: (width: number) => void;

  setArtifactPanelOpen: (open: boolean) => void;

  setCommandPaletteOpen: (open: boolean) => void;
  toggleCommandPalette: () => void;

}

export const useUIStore = create<UIState>()(
  persist(
    (set) => ({
      sidebarCollapsed: false,
      sidebarWidth: 264,
      artifactPanelOpen: false,
      commandPaletteOpen: false,

      toggleSidebar: () => set((s) => ({ sidebarCollapsed: !s.sidebarCollapsed })),
      setSidebarWidth: (width) => set({ sidebarWidth: width }),

      setArtifactPanelOpen: (open) => set({ artifactPanelOpen: open }),

      setCommandPaletteOpen: (open) => set({ commandPaletteOpen: open }),
      toggleCommandPalette: () =>
        set((s) => ({ commandPaletteOpen: !s.commandPaletteOpen })),

    }),
    {
      name: "jkcodingagent:ui",
      partialize: (s) => ({
        sidebarCollapsed: s.sidebarCollapsed,
        sidebarWidth: s.sidebarWidth,
      }),
    },
  ),
);

import { lazy, Suspense, useState } from "react";
import { Settings, Moon, Sun } from "lucide-react";
import type { ThemeMode } from "../types";
import { NotificationBell } from "./NotificationBell";
import { UsagePopover } from "./UsagePopover";
import s from "../styles";

const AppSettingsDialog = lazy(() =>
  import("./AppSettingsDialog").then((module) => ({ default: module.AppSettingsDialog })),
);

export function SidebarFooterActions({
  isDark,
  themeMode,
  systemPrefersDark,
  onThemeModeChange,
  onToggleTheme,
  projectPath,
}: {
  isDark: boolean;
  themeMode: ThemeMode;
  systemPrefersDark: boolean;
  onThemeModeChange: (mode: ThemeMode) => void;
  onToggleTheme: () => void;
  projectPath?: string;
}) {
  const [showAppSettings, setShowAppSettings] = useState(false);

  return (
    <>
      <div style={s.sidebarFooterActions}>
        <NotificationBell />
        <button
          style={s.sidebarIconBtn}
          title="应用设置"
          onClick={() => setShowAppSettings(true)}
        >
          <Settings size={14} strokeWidth={1.6} color="var(--text-hint)" />
        </button>
        <button
          style={s.sidebarIconBtn}
          title={isDark ? "切换到浅色模式" : "切换到深色模式"}
          onClick={onToggleTheme}
        >
          {isDark ? (
            <Sun size={14} strokeWidth={1.8} color="var(--text-hint)" />
          ) : (
            <Moon size={14} strokeWidth={1.8} color="var(--text-hint)" />
          )}
        </button>
        <UsagePopover />
      </div>

      {showAppSettings && (
        <Suspense fallback={null}>
          <AppSettingsDialog
            isDark={isDark}
            themeMode={themeMode}
            systemPrefersDark={systemPrefersDark}
            onThemeModeChange={onThemeModeChange}
            projectPath={projectPath}
            onClose={() => setShowAppSettings(false)}
          />
        </Suspense>
      )}
    </>
  );
}

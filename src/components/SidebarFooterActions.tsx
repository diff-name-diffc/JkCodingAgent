import { lazy, Suspense, useState } from "react";
import { Settings } from "lucide-react";
import { NotificationBell } from "./NotificationBell";
import { UsagePopover } from "./UsagePopover";

const AppSettingsDialog = lazy(() =>
  import("./AppSettingsDialog").then((module) => ({ default: module.AppSettingsDialog })),
);

export function SidebarFooterActions({
  projectId,
  projectPath,
}: {
  projectId?: string;
  projectPath?: string;
} = {}) {
  const [showAppSettings, setShowAppSettings] = useState(false);

  return (
    <>
      <div className="ai-sidebar-footer-actions">
        <NotificationBell />
        <button
          className="ai-sidebar-footer-button"
          title="应用设置"
          onClick={() => setShowAppSettings(true)}
        >
          <Settings size={14} strokeWidth={1.6} color="var(--text-hint)" />
        </button>
        <UsagePopover />
      </div>

      {showAppSettings && (
        <Suspense fallback={null}>
          <AppSettingsDialog
            projectId={projectId}
            projectPath={projectPath}
            onClose={() => setShowAppSettings(false)}
          />
        </Suspense>
      )}
    </>
  );
}

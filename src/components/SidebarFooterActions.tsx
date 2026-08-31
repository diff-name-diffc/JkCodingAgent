import { lazy, Suspense, useState } from "react";
import * as Popover from "@radix-ui/react-popover";
import { MoreHorizontal, Settings } from "lucide-react";
import { NotificationBell } from "./NotificationBell";

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
      <Popover.Root>
        <Popover.Trigger asChild>
          <button
            type="button"
            className="ai-sidebar-footer-menu-trigger"
            aria-label="打开通知与设置"
            title="更多"
          >
            <MoreHorizontal size={18} strokeWidth={1.8} />
          </button>
        </Popover.Trigger>
        <Popover.Portal>
          <Popover.Content
            side="right"
            align="end"
            sideOffset={10}
            className="ai-sidebar-footer-menu"
          >
            <div className="ai-sidebar-footer-menu-title">应用菜单</div>
            <div className="ai-sidebar-footer-actions">
              <div className="ai-sidebar-footer-action">
                <NotificationBell />
                <span>通知</span>
              </div>
              <div className="ai-sidebar-footer-action">
                <button
                  title="应用设置"
                  onClick={() => setShowAppSettings(true)}
                >
                  <Settings size={14} strokeWidth={1.8} />
                </button>
                <span>设置</span>
              </div>
            </div>
            <Popover.Arrow className="ai-sidebar-footer-menu-arrow" />
          </Popover.Content>
        </Popover.Portal>
      </Popover.Root>

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

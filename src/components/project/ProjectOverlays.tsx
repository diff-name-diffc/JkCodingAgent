import { lazy, Suspense } from "react";
import type { McpStatus, ModelCategory, Project } from "../../types";

const AppSettingsDialog = lazy(() =>
  import("../AppSettingsDialog").then((module) => ({ default: module.AppSettingsDialog })),
);
const McpStatusDialog = lazy(() =>
  import("../McpStatusDialog").then((module) => ({ default: module.McpStatusDialog })),
);
interface ProjectOverlaysProps {
  project: Project;
  showSettings: boolean;
  /** 「配置模型」深链携带的模型服务页初始分类（UI-25 遗留）；null 走缺省。 */
  settingsProvidersCategory?: ModelCategory | null;
  showMcpStatus: boolean;
  mcpStatus: McpStatus | null;
  mcpChecking: boolean;
  mcpUpdatingServer: string | null;
  onCloseSettings: () => void;
  onCloseMcpStatus: () => void;
  onRefreshMcpStatus: () => void;
  onToggleMcpServer: (serverName: string, enabled: boolean) => void;
}

export function ProjectOverlays({
  project,
  showSettings,
  settingsProvidersCategory,
  showMcpStatus,
  mcpStatus,
  mcpChecking,
  mcpUpdatingServer,
  onCloseSettings,
  onCloseMcpStatus,
  onRefreshMcpStatus,
  onToggleMcpServer,
}: ProjectOverlaysProps) {
  return (
    <>
      {showSettings && (
        <Suspense fallback={null}>
          <AppSettingsDialog
            initialTab="providers"
            initialProvidersCategory={settingsProvidersCategory ?? undefined}
            projectId={project.id}
            projectPath={project.path}
            onClose={onCloseSettings}
          />
        </Suspense>
      )}

      {showMcpStatus && (
        <Suspense fallback={null}>
          <McpStatusDialog
            scope="project"
            status={mcpStatus}
            checking={mcpChecking}
            updatingServer={mcpUpdatingServer}
            onRefresh={onRefreshMcpStatus}
            onToggleServerEnabled={onToggleMcpServer}
            onClose={onCloseMcpStatus}
          />
        </Suspense>
      )}
    </>
  );
}

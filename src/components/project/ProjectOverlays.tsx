import { lazy, Suspense } from "react";
import type { DockedBrowser } from "../BrowserDock";
import type { McpStatus, Project } from "../../types";

const AppSettingsDialog = lazy(() =>
  import("../AppSettingsDialog").then((module) => ({ default: module.AppSettingsDialog })),
);
const McpStatusDialog = lazy(() =>
  import("../McpStatusDialog").then((module) => ({ default: module.McpStatusDialog })),
);
const BrowserDock = lazy(() =>
  import("../BrowserDock").then((module) => ({ default: module.BrowserDock })),
);

interface ProjectOverlaysProps {
  project: Project;
  showSettings: boolean;
  showMcpStatus: boolean;
  mcpStatus: McpStatus | null;
  mcpChecking: boolean;
  mcpUpdatingServer: string | null;
  dockedSessions: DockedBrowser[];
  onCloseSettings: () => void;
  onCloseMcpStatus: () => void;
  onRefreshMcpStatus: () => void;
  onToggleMcpServer: (serverName: string, enabled: boolean) => void;
  onRestoreBrowser: (sessionId: string) => void | Promise<void>;
  onCloseBrowser: (sessionId: string) => void | Promise<void>;
}

export function ProjectOverlays({
  project,
  showSettings,
  showMcpStatus,
  mcpStatus,
  mcpChecking,
  mcpUpdatingServer,
  dockedSessions,
  onCloseSettings,
  onCloseMcpStatus,
  onRefreshMcpStatus,
  onToggleMcpServer,
  onRestoreBrowser,
  onCloseBrowser,
}: ProjectOverlaysProps) {
  return (
    <>
      {showSettings && (
        <Suspense fallback={null}>
          <AppSettingsDialog
            initialTab="providers"
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

      {dockedSessions.length > 0 && (
        <Suspense fallback={null}>
          <BrowserDock
            sessions={dockedSessions}
            onRestore={onRestoreBrowser}
            onClose={onCloseBrowser}
          />
        </Suspense>
      )}
    </>
  );
}

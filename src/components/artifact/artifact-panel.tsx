import * as React from "react";
import { invoke } from "@tauri-apps/api/core";
import { AlertCircle, FileSearch, Loader2, X } from "lucide-react";
import type { DispatcherToolArtifact, DispatcherToolArtifactRef } from "../../types";
import type { SubAgentSession } from "../subAgentEventStore";
import { useUIStore } from "../../stores/ui-store";
import { cn } from "../../lib/cn";
import { Button } from "../ui/button";
import { ScrollArea } from "../ui/scroll-area";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "../ui/tabs";
import { SubAgentExecutionCard } from "../SubAgentExecutionView";

/**
 * Optional right-side detail panel for the refactored chat surface.
 *
 * Reserved for: agent tool-call detail, generated document/code preview,
 * citation sources, file preview, task execution trace. Today it renders a
 * generic tabbed shell; concrete content is passed in as children/props by
 * the orchestrator.
 */
export interface ArtifactPanelProps {
  title?: string;
  workspaceId?: string | null;
  artifact?: DispatcherToolArtifactRef | null;
  subAgentSession?: SubAgentSession | null;
  traceLoading?: boolean;
  traceError?: string | null;
  /** Tab labels → content. */
  tabs?: { label: string; value: string; content: React.ReactNode }[];
  /** If no tabs are provided, render this single content node. */
  children?: React.ReactNode;
  className?: string;
}

export function ArtifactPanel({
  title = "详情",
  workspaceId,
  artifact,
  subAgentSession,
  traceLoading = false,
  traceError,
  tabs,
  children,
  className,
}: ArtifactPanelProps) {
  const setArtifactPanelOpen = useUIStore((s) => s.setArtifactPanelOpen);
  const [loadedArtifact, setLoadedArtifact] =
    React.useState<DispatcherToolArtifact | null>(null);
  const [loading, setLoading] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);

  React.useEffect(() => {
    if (!workspaceId || !artifact) {
      setLoadedArtifact(null);
      setLoading(false);
      setError(null);
      return;
    }

    let cancelled = false;
    setLoading(true);
    setError(null);
    setLoadedArtifact(null);

    invoke<DispatcherToolArtifact>("dispatcher_get_tool_artifact", {
      workspaceId,
      artifactId: artifact.id,
    })
      .then((loaded) => {
        if (!cancelled) setLoadedArtifact(loaded);
      })
      .catch((loadError) => {
        if (!cancelled) {
          setError(loadError instanceof Error ? loadError.message : String(loadError));
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [artifact, workspaceId]);

  const panelTitle = artifact?.title ?? title;

  return (
    <div className={cn("flex h-full flex-col", className)}>
      <div className="flex items-center gap-2 border-b border-border px-4 py-2.5">
        <h3 className="flex-1 truncate text-sm font-semibold text-foreground">
          {panelTitle}
        </h3>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="关闭详情面板"
          onClick={() => setArtifactPanelOpen(false)}
        >
          <X className="h-4 w-4" />
        </Button>
      </div>

      <ScrollArea className="min-h-0 flex-1">
        <div className="p-4">
          {subAgentSession ? (
            <SubAgentExecutionCard session={subAgentSession} />
          ) : traceLoading ? (
            <div className="flex items-center gap-2 rounded-lg border border-border bg-muted/30 px-3 py-2 text-sm text-muted-foreground">
              <Loader2 className="h-4 w-4 animate-spin" />
              正在加载执行轨迹...
            </div>
          ) : traceError ? (
            <div className="flex items-start gap-2 rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
              <AlertCircle className="mt-0.5 h-4 w-4 shrink-0" />
              <span>{traceError}</span>
            </div>
          ) : artifact ? (
            <ToolArtifactContent
              artifact={artifact}
              loadedArtifact={loadedArtifact}
              loading={loading}
              error={error}
            />
          ) : tabs && tabs.length > 0 ? (
            <Tabs defaultValue={tabs[0].value} className="w-full">
              <TabsList className="mb-3">
                {tabs.map((tab) => (
                  <TabsTrigger key={tab.value} value={tab.value}>
                    {tab.label}
                  </TabsTrigger>
                ))}
              </TabsList>
              {tabs.map((tab) => (
                <TabsContent key={tab.value} value={tab.value}>
                  {tab.content}
                </TabsContent>
              ))}
            </Tabs>
          ) : (
            children ?? <ArtifactEmptyState />
          )}
        </div>
      </ScrollArea>
    </div>
  );
}

function ToolArtifactContent({
  artifact,
  loadedArtifact,
  loading,
  error,
}: {
  artifact: DispatcherToolArtifactRef;
  loadedArtifact: DispatcherToolArtifact | null;
  loading: boolean;
  error: string | null;
}) {
  return (
    <div className="space-y-3">
      <div className="rounded-lg border border-border bg-card/70 p-3">
        <div className="mb-1 flex items-center gap-2">
          <FileSearch className="h-4 w-4 text-primary" />
          <div className="min-w-0 flex-1">
            <div className="truncate text-sm font-medium text-foreground">
              {artifact.title}
            </div>
            <div className="text-[11px] text-muted-foreground">
              {artifact.kind} · {artifact.lineCount} 行 · {artifact.charCount} 字符
            </div>
          </div>
        </div>
        {artifact.preview && (
          <pre className="chat-scroll mt-2 max-h-28 overflow-auto rounded-md bg-muted/50 p-2 font-mono text-[11px] leading-relaxed text-muted-foreground">
            {artifact.preview}
          </pre>
        )}
      </div>

      {loading && (
        <div className="flex items-center gap-2 rounded-lg border border-border bg-muted/30 px-3 py-2 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          正在加载工具产物...
        </div>
      )}

      {error && (
        <div className="flex items-start gap-2 rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
          <AlertCircle className="mt-0.5 h-4 w-4 shrink-0" />
          <span>{error}</span>
        </div>
      )}

      {loadedArtifact && (
        <pre className="chat-scroll min-h-40 overflow-auto rounded-lg border border-border bg-background p-3 font-mono text-[12px] leading-relaxed text-foreground">
          {loadedArtifact.content}
        </pre>
      )}
    </div>
  );
}

function ArtifactEmptyState() {
  return (
    <div className="flex min-h-48 flex-col items-center justify-center rounded-lg border border-dashed border-border bg-muted/20 px-4 text-center">
      <FileSearch className="mb-2 h-5 w-5 text-muted-foreground" />
      <div className="text-sm font-medium text-foreground">暂无详情</div>
      <div className="mt-1 text-xs text-muted-foreground">
        从工具调用中打开详细结果后会显示在这里。
      </div>
    </div>
  );
}

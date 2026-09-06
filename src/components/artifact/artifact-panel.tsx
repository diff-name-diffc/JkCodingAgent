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
import { DetailSection } from "../detail/DetailSection";
import { OutputBlock } from "../detail/OutputBlock";
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
  // UI-14：元信息分区与输出块接入共享详情视觉（DetailSection/OutputBlock）。
  return (
    <div className="space-y-3">
      <DetailSection
        title={
          <span className="flex min-w-0 items-center gap-2">
            <FileSearch className="h-4 w-4 shrink-0 text-primary" />
            <span className="truncate">{artifact.title}</span>
          </span>
        }
      >
        <div className="ai-detail-meta-grid">
          <div className="ai-detail-meta-item">
            <span className="ai-detail-meta-label">类型</span>
            <span className="ai-detail-meta-value">{artifact.kind}</span>
          </div>
          <div className="ai-detail-meta-item">
            <span className="ai-detail-meta-label">行数</span>
            <span className="ai-detail-meta-value">{artifact.lineCount}</span>
          </div>
          <div className="ai-detail-meta-item">
            <span className="ai-detail-meta-label">字符数</span>
            <span className="ai-detail-meta-value">{artifact.charCount}</span>
          </div>
        </div>
        {artifact.preview && <OutputBlock text={artifact.preview} className="mt-2 max-h-28" />}
      </DetailSection>

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
        <OutputBlock
          text={loadedArtifact.content}
          emptyHint="产物内容为空"
          className="min-h-40 rounded-lg border border-border bg-background p-3 text-foreground"
        />
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

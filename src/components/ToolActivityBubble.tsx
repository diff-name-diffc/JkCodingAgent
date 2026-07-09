import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChevronDown, ChevronRight, FileSearch, LoaderCircle, Wrench } from "lucide-react";
import { MarkdownRenderer } from "./markdown/MarkdownRenderer";
import { SubAgentExecutionCard } from "./SubAgentExecutionView";
import { extractAgentIdsFromToolInput, useSubAgentSessions } from "./subAgentEventStore";
import type { SubAgentSession } from "./subAgentEventStore";
import { formatTokenGenerationSpeed } from "./dispatcher-chat/dispatcherChatUtils";
import type {
  DispatcherToolArtifact,
  DispatcherToolArtifactRef,
  DispatcherToolResultMode,
} from "../types";

export interface ToolActivityItem {
  key: string;
  name: string;
  input?: string;
  displayText?: string;
  detailRefs?: DispatcherToolArtifactRef[];
  resultMode?: DispatcherToolResultMode;
  status: "planned" | "running" | "completed";
  summaryText?: string;
}

/**
 * Tracks live elapsed time for a running sub-agent session. Returns the
 * elapsed_ms from the last UsageUpdated event plus the wall-clock delta
 * since that event was received, so the token speed ticks live.
 */
function useSubAgentLiveElapsed(session: SubAgentSession | null): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!session || session.status !== "running" || session.usageReceivedAt == null) {
      return;
    }
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [session]);

  if (!session || session.usageReceivedAt == null) {
    return session?.elapsed ?? 0;
  }
  return session.elapsed + Math.max(0, now - session.usageReceivedAt);
}

function SubAgentLiveSpeed({ session }: { session: SubAgentSession }) {
  const liveElapsed = useSubAgentLiveElapsed(session);
  const completionTokens = session.tokenUsage?.completionTokens ?? 0;
  const speed = formatTokenGenerationSpeed(completionTokens, liveElapsed);

  if (completionTokens <= 0) return null;

  return <span className="ai-tool-activity-sub-agent-speed">{speed} t/s</span>;
}

interface ToolActivityBubbleProps {
  tools: ToolActivityItem[];
  workspaceId: string;
  title?: string;
}

export function ToolActivityBubble({
  tools,
  workspaceId,
  title = "工具活动",
}: ToolActivityBubbleProps) {
  const [expandedTools, setExpandedTools] = useState<Record<string, boolean>>({});
  const [activeArtifactByTool, setActiveArtifactByTool] = useState<Record<string, string | null>>(
    {},
  );
  const [artifactCache, setArtifactCache] = useState<Record<string, DispatcherToolArtifact>>({});
  const [artifactLoading, setArtifactLoading] = useState<Record<string, boolean>>({});
  const [artifactErrors, setArtifactErrors] = useState<Record<string, string>>({});
  const subAgentSessions = useSubAgentSessions(workspaceId);

  const runningCount = useMemo(
    () => tools.filter((tool) => tool.status === "running").length,
    [tools],
  );
  const plannedCount = useMemo(
    () => tools.filter((tool) => tool.status === "planned").length,
    [tools],
  );

  if (tools.length === 0) {
    return null;
  }

  const toggleTool = (key: string) => {
    setExpandedTools((prev) => ({ ...prev, [key]: !prev[key] }));
  };

  const handleOpenArtifact = async (toolKey: string, artifact: DispatcherToolArtifactRef) => {
    const alreadyActive = activeArtifactByTool[toolKey] === artifact.id;
    if (alreadyActive) {
      setActiveArtifactByTool((prev) => ({ ...prev, [toolKey]: null }));
      return;
    }

    setActiveArtifactByTool((prev) => ({ ...prev, [toolKey]: artifact.id }));
    if (artifactCache[artifact.id] || artifactLoading[artifact.id]) {
      return;
    }

    setArtifactLoading((prev) => ({ ...prev, [artifact.id]: true }));
    setArtifactErrors((prev) => {
      const next = { ...prev };
      delete next[artifact.id];
      return next;
    });

    try {
      const loaded = await invoke<DispatcherToolArtifact>("dispatcher_get_tool_artifact", {
        workspaceId,
        artifactId: artifact.id,
      });
      setArtifactCache((prev) => ({ ...prev, [artifact.id]: loaded }));
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setArtifactErrors((prev) => ({ ...prev, [artifact.id]: message }));
    } finally {
      setArtifactLoading((prev) => ({ ...prev, [artifact.id]: false }));
    }
  };

  return (
    <div className="ai-tool-activity ai-migrated-tool-activity">
      <div className="ai-tool-activity-header">
        <div className="ai-tool-activity-header-title">
          <Wrench size={13} color="var(--text-secondary)" />
          <span>{title}</span>
        </div>
        <div className="ai-tool-activity-header-meta">
          <span className="ai-tool-activity-count">{tools.length}</span>
          {plannedCount > 0 && (
            <span className="ai-tool-activity-badge is-planned">待执行 {plannedCount}</span>
          )}
          {runningCount > 0 && (
            <span className="ai-tool-activity-badge is-running">运行中 {runningCount}</span>
          )}
        </div>
      </div>
      <div className="ai-tool-activity-list">
        {tools.map((tool, index) => {
          const expanded = expandedTools[tool.key] ?? false;
          const activeArtifactId = activeArtifactByTool[tool.key] ?? null;
          const activeArtifact = activeArtifactId ? artifactCache[activeArtifactId] : undefined;
          const activeArtifactError = activeArtifactId ? artifactErrors[activeArtifactId] : null;
          const activeArtifactLoading = activeArtifactId
            ? artifactLoading[activeArtifactId]
            : false;
          const detailRefs = tool.detailRefs ?? [];
          const hasDisplayText = Boolean(tool.displayText?.trim());
          const showSummaryInConversation = shouldDisplaySummaryInConversation(tool.resultMode);
          const subAgentId =
            tool.name === "call_sub_agent" ? extractAgentIdsFromToolInput(tool.input) : null;
          const subAgentSession = subAgentId ? subAgentSessions[subAgentId] : null;

          return (
            <div
              key={tool.key}
              className={`ai-tool-activity-item${index === 0 ? " is-first" : ""}`}
            >
              <button
                type="button"
                className="ai-tool-activity-toggle"
                onClick={() => toggleTool(tool.key)}
              >
                <div className="ai-tool-activity-title-row">
                  {expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                  <span className={`ai-tool-activity-status-dot ${toolStatusClass(tool.status)}`} />
                  <span className="ai-tool-activity-name">{tool.name}</span>
                  {tool.resultMode && (
                    <span className={`ai-tool-activity-mode ${toolModeClass(tool.resultMode)}`}>
                      {toolResultModeLabel(tool.resultMode)}
                    </span>
                  )}
                  {detailRefs.length > 0 && (
                    <span className="ai-tool-activity-ref-badge">详细引用 {detailRefs.length}</span>
                  )}
                  {subAgentSession && (
                    <span
                      className={`ai-tool-activity-sub-agent-badge ${subAgentStatusClass(
                        subAgentSession.status,
                      )}`}
                    >
                      子智能体{formatSubAgentStatus(subAgentSession)}
                    </span>
                  )}
                </div>
                <div className="ai-tool-activity-right">
                  {hasDisplayText && (
                    <span className="ai-tool-activity-preview" title={tool.displayText}>
                      {tool.displayText}
                    </span>
                  )}
                  {subAgentSession && subAgentSession.status === "running" && subAgentSession.tokenUsage && (
                    <SubAgentLiveSpeed session={subAgentSession} />
                  )}
                  <span className={`ai-tool-activity-status ${toolStatusClass(tool.status)}`}>
                    {tool.status === "running"
                      ? "执行中"
                      : tool.status === "planned"
                        ? "待执行"
                        : "已完成"}
                  </span>
                </div>
              </button>
              {expanded && (
                <div className="ai-tool-activity-body">
                  {tool.input?.trim() && (
                    <div className="ai-tool-activity-block">
                      <div className="ai-tool-activity-block-label">调用参数</div>
                      <pre className="ai-tool-activity-code session-selectable">
                        {tool.input}
                      </pre>
                    </div>
                  )}

                  {hasDisplayText && (
                    <div className="ai-tool-activity-block">
                      <div className="ai-tool-activity-block-label">
                        {tool.name === "local_zsh" ? "执行结果与审计历史" : "结果"}
                      </div>
                      {tool.name === "local_zsh" ? (
                        <div className="ai-tool-activity-summary">
                          <MarkdownRenderer content={tool.displayText ?? ""} variant="chat" />
                        </div>
                      ) : (
                        <pre className="ai-tool-activity-code session-selectable">
                          {tool.displayText}
                        </pre>
                      )}
                    </div>
                  )}

                  {showSummaryInConversation && (
                    <div className="ai-tool-activity-block">
                      <div className="ai-tool-activity-block-label">结果摘要</div>
                      {tool.summaryText ? (
                        <div className="ai-tool-activity-summary">
                          <MarkdownRenderer content={tool.summaryText} variant="chat" />
                        </div>
                      ) : (
                        <div className="ai-tool-activity-info">摘要内容将在处理完成后展示。</div>
                      )}
                    </div>
                  )}

                  {subAgentSession && (
                    <div className="ai-tool-activity-block">
                      <div className="ai-tool-activity-block-label">子智能体运行状态</div>
                      <SubAgentExecutionCard session={subAgentSession} autoExpand={false} />
                    </div>
                  )}

                  {detailRefs.length > 0 && (
                    <div className="ai-tool-activity-block">
                      <div className="ai-tool-activity-block-label">详细结果引用</div>
                      <div className="ai-tool-activity-ref-list">
                        {detailRefs.map((artifact) => {
                          const selected = activeArtifactId === artifact.id;
                          return (
                            <button
                              key={artifact.id}
                              type="button"
                              className={`ai-tool-activity-ref-button${
                                selected ? " is-active" : ""
                              }`}
                              onClick={() => handleOpenArtifact(tool.key, artifact)}
                            >
                              <div className="ai-tool-activity-ref-title">
                                <FileSearch size={13} />
                                <span>{artifact.title}</span>
                              </div>
                              <div className="ai-tool-activity-ref-meta">
                                {artifact.lineCount} 行 · {artifact.charCount} 字符
                              </div>
                              <div className="ai-tool-activity-ref-preview">{artifact.preview}</div>
                            </button>
                          );
                        })}
                      </div>
                    </div>
                  )}

                  {!hasDisplayText && detailRefs.length === 0 && !showSummaryInConversation && (
                    <div className="ai-tool-activity-pending">
                      {tool.status === "planned"
                        ? "等待开始执行..."
                        : tool.status === "running"
                          ? "等待工具返回..."
                          : "工具未返回可展示内容"}
                    </div>
                  )}

                  {activeArtifactId && (
                    <div className="ai-tool-activity-block">
                      <div className="ai-tool-activity-block-label">详细结果内容</div>
                      {activeArtifactLoading ? (
                        <div className="ai-tool-activity-loading">
                          <LoaderCircle size={14} className="ai-tool-activity-spin" />
                          正在加载详细结果...
                        </div>
                      ) : activeArtifactError ? (
                        <div className="ai-tool-activity-error">{activeArtifactError}</div>
                      ) : activeArtifact ? (
                        <pre className="ai-tool-activity-code session-selectable">
                          {activeArtifact.content}
                        </pre>
                      ) : (
                        <div className="ai-tool-activity-pending">详细结果尚未加载</div>
                      )}
                    </div>
                  )}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function formatSubAgentStatus(session: { status: "running" | "completed" | "failed" }): string {
  if (session.status === "running") return "执行中";
  if (session.status === "failed") return "失败";
  return "完成";
}

function subAgentStatusClass(status: "running" | "completed" | "failed"): string {
  if (status === "running") return "is-running";
  if (status === "failed") return "is-failed";
  return "is-completed";
}

function toolResultModeLabel(mode: DispatcherToolResultMode): string {
  switch (mode) {
    case "summary":
      return "摘要";
    case "conservative_summary":
      return "高保真压缩";
    case "intent_compressed":
      return "语义压缩";
    case "structured_fallback":
      return "结构化提取";
    case "truncated":
      return "已截断";
    case "raw":
    default:
      return "原文";
  }
}

function toolModeClass(mode: DispatcherToolResultMode): string {
  if (mode === "summary" || mode === "intent_compressed") return "is-accent";
  if (mode === "conservative_summary" || mode === "structured_fallback") return "is-warning";
  if (mode === "truncated") return "is-danger";
  return "is-raw";
}

function toolStatusClass(status: ToolActivityItem["status"]): string {
  if (status === "running") return "is-running";
  if (status === "planned") return "is-planned";
  return "is-completed";
}

function shouldDisplaySummaryInConversation(mode: DispatcherToolResultMode | undefined): boolean {
  return (
    mode === "summary" ||
    mode === "conservative_summary" ||
    mode === "intent_compressed" ||
    mode === "structured_fallback"
  );
}

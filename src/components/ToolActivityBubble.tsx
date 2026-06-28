import { useEffect, useMemo, useState } from "react";
import type { CSSProperties } from "react";
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

  return (
    <span style={styles.subAgentSpeed}>
      {speed} t/s
    </span>
  );
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
    <div style={styles.container}>
      <div style={styles.header}>
        <div style={styles.headerTitleWrap}>
          <Wrench size={13} color="var(--text-secondary)" />
          <span style={styles.headerTitle}>{title}</span>
        </div>
        <div style={styles.headerMeta}>
          <span style={styles.headerCount}>{tools.length}</span>
          {plannedCount > 0 && <span style={styles.plannedBadge}>待执行 {plannedCount}</span>}
          {runningCount > 0 && <span style={styles.runningBadge}>运行中 {runningCount}</span>}
        </div>
      </div>
      <div style={styles.list}>
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
              style={{
                ...styles.item,
                borderTop: index === 0 ? "none" : "1px solid var(--border-dim)",
              }}
            >
              <button type="button" style={styles.itemToggle} onClick={() => toggleTool(tool.key)}>
                <div style={styles.itemTitleWrap}>
                  {expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                  <span
                    style={{
                      ...styles.statusDot,
                      background:
                        tool.status === "running"
                          ? "var(--success)"
                          : tool.status === "planned"
                            ? "var(--warning, #d97706)"
                            : "var(--text-hint)",
                    }}
                  />
                  <span style={styles.itemTitle}>{tool.name}</span>
                  {tool.resultMode && (
                    <span style={toolModeBadgeStyle(tool.resultMode)}>
                      {toolResultModeLabel(tool.resultMode)}
                    </span>
                  )}
                  {detailRefs.length > 0 && (
                    <span style={styles.refBadge}>详细引用 {detailRefs.length}</span>
                  )}
                  {subAgentSession && (
                    <span style={subAgentStatusStyle(subAgentSession.status)}>
                      子智能体{formatSubAgentStatus(subAgentSession)}
                    </span>
                  )}
                </div>
                <div style={styles.itemRightWrap}>
                  {hasDisplayText && (
                    <span style={styles.itemDisplayText} title={tool.displayText}>
                      {tool.displayText}
                    </span>
                  )}
                  {subAgentSession && subAgentSession.status === "running" && subAgentSession.tokenUsage && (
                    <SubAgentLiveSpeed session={subAgentSession} />
                  )}
                  <span
                    style={{
                      ...styles.itemStatus,
                      color:
                        tool.status === "running"
                          ? "var(--success)"
                          : tool.status === "planned"
                            ? "var(--warning, #d97706)"
                            : "var(--text-secondary)",
                    }}
                  >
                    {tool.status === "running"
                      ? "执行中"
                      : tool.status === "planned"
                        ? "待执行"
                        : "已完成"}
                  </span>
                </div>
              </button>
              {expanded && (
                <div style={styles.itemBody}>
                  {tool.input?.trim() && (
                    <div style={styles.block}>
                      <div style={styles.blockLabel}>调用参数</div>
                      <pre className="session-selectable" style={styles.blockCode}>
                        {tool.input}
                      </pre>
                    </div>
                  )}

                  {hasDisplayText && (
                    <div style={styles.block}>
                      <div style={styles.blockLabel}>
                        {tool.name === "local_zsh" ? "执行结果与审计历史" : "结果"}
                      </div>
                      {tool.name === "local_zsh" ? (
                        <div style={styles.summaryContent}>
                          <MarkdownRenderer content={tool.displayText ?? ""} variant="chat" />
                        </div>
                      ) : (
                        <pre className="session-selectable" style={styles.blockCode}>
                          {tool.displayText}
                        </pre>
                      )}
                    </div>
                  )}

                  {showSummaryInConversation && (
                    <div style={styles.block}>
                      <div style={styles.blockLabel}>结果摘要</div>
                      {tool.summaryText ? (
                        <div style={styles.summaryContent}>
                          <MarkdownRenderer content={tool.summaryText} variant="chat" />
                        </div>
                      ) : (
                        <div style={styles.infoState}>摘要内容将在处理完成后展示。</div>
                      )}
                    </div>
                  )}

                  {subAgentSession && (
                    <div style={styles.block}>
                      <div style={styles.blockLabel}>子智能体运行状态</div>
                      <SubAgentExecutionCard session={subAgentSession} autoExpand={false} />
                    </div>
                  )}

                  {detailRefs.length > 0 && (
                    <div style={styles.block}>
                      <div style={styles.blockLabel}>详细结果引用</div>
                      <div style={styles.refList}>
                        {detailRefs.map((artifact) => {
                          const selected = activeArtifactId === artifact.id;
                          return (
                            <button
                              key={artifact.id}
                              type="button"
                              style={{
                                ...styles.refButton,
                                ...(selected ? styles.refButtonActive : {}),
                              }}
                              onClick={() => handleOpenArtifact(tool.key, artifact)}
                            >
                              <div style={styles.refButtonHeader}>
                                <FileSearch size={13} />
                                <span style={styles.refButtonTitle}>{artifact.title}</span>
                              </div>
                              <div style={styles.refButtonMeta}>
                                {artifact.lineCount} 行 · {artifact.charCount} 字符
                              </div>
                              <div style={styles.refButtonPreview}>{artifact.preview}</div>
                            </button>
                          );
                        })}
                      </div>
                    </div>
                  )}

                  {!hasDisplayText && detailRefs.length === 0 && !showSummaryInConversation && (
                    <div style={styles.pendingText}>
                      {tool.status === "planned"
                        ? "等待开始执行..."
                        : tool.status === "running"
                          ? "等待工具返回..."
                          : "工具未返回可展示内容"}
                    </div>
                  )}

                  {activeArtifactId && (
                    <div style={styles.block}>
                      <div style={styles.blockLabel}>详细结果内容</div>
                      {activeArtifactLoading ? (
                        <div style={styles.loadingState}>
                          <LoaderCircle size={14} style={styles.loadingIcon} />
                          正在加载详细结果...
                        </div>
                      ) : activeArtifactError ? (
                        <div style={styles.errorState}>{activeArtifactError}</div>
                      ) : activeArtifact ? (
                        <pre className="session-selectable" style={styles.blockCode}>
                          {activeArtifact.content}
                        </pre>
                      ) : (
                        <div style={styles.pendingText}>详细结果尚未加载</div>
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

function subAgentStatusStyle(status: "running" | "completed" | "failed"): CSSProperties {
  const color =
    status === "running"
      ? "var(--accent)"
      : status === "failed"
        ? "var(--danger, #dc2626)"
        : "var(--success)";
  return {
    ...styles.subAgentBadge,
    color,
    borderColor: `color-mix(in srgb, ${color} 28%, transparent)`,
    background: `color-mix(in srgb, ${color} 10%, transparent)`,
  };
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

function toolModeBadgeStyle(mode: DispatcherToolResultMode): CSSProperties {
  const accent =
    mode === "summary" || mode === "intent_compressed"
      ? "var(--accent)"
      : mode === "conservative_summary" || mode === "structured_fallback"
        ? "var(--warning, #d97706)"
        : mode === "truncated"
          ? "var(--danger, #dc2626)"
          : "var(--text-hint)";
  const background =
    mode === "summary" || mode === "intent_compressed"
      ? "color-mix(in srgb, var(--accent) 12%, transparent)"
      : mode === "conservative_summary" || mode === "structured_fallback"
        ? "rgba(217,119,6,0.12)"
        : mode === "truncated"
          ? "rgba(220,38,38,0.12)"
          : "var(--bg-hover)";

  return {
    ...styles.modeBadge,
    color: accent,
    background,
    borderColor: `color-mix(in srgb, ${accent} 24%, transparent)`,
  };
}

function shouldDisplaySummaryInConversation(mode: DispatcherToolResultMode | undefined): boolean {
  return (
    mode === "summary" ||
    mode === "conservative_summary" ||
    mode === "intent_compressed" ||
    mode === "structured_fallback"
  );
}

const styles = {
  container: {
    width: "100%",
    border: "1px solid color-mix(in srgb, var(--accent) 16%, var(--chat-glass-border))",
    borderRadius: 18,
    background:
      "linear-gradient(180deg, color-mix(in srgb, var(--chat-surface-strong) 90%, transparent), color-mix(in srgb, var(--chat-surface) 86%, transparent))",
    boxShadow: "var(--chat-shadow-soft)",
    overflow: "hidden",
  },
  header: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: 12,
    padding: "10px 14px",
    background:
      "linear-gradient(90deg, color-mix(in srgb, var(--accent) 10%, transparent), transparent)",
    borderBottom: "1px solid var(--chat-glass-border)",
  },
  headerTitleWrap: {
    display: "flex",
    alignItems: "center",
    gap: 8,
  },
  headerTitle: {
    fontSize: 12,
    fontWeight: 700,
    color: "var(--text-secondary)",
    letterSpacing: 0.2,
    textTransform: "uppercase" as const,
  },
  headerMeta: {
    display: "flex",
    alignItems: "center",
    gap: 8,
  },
  headerCount: {
    minWidth: 22,
    height: 22,
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    padding: "0 7px",
    borderRadius: 999,
    background: "var(--chat-surface-strong)",
    border: "1px solid var(--chat-glass-border)",
    fontSize: 11,
    fontWeight: 700,
    color: "var(--text-secondary)",
    fontFamily: "var(--font-mono)",
  },
  runningBadge: {
    padding: "3px 8px",
    borderRadius: 999,
    background: "color-mix(in srgb, var(--success) 14%, transparent)",
    color: "var(--success)",
    fontSize: 11,
    fontWeight: 700,
  },
  plannedBadge: {
    padding: "3px 8px",
    borderRadius: 999,
    background: "rgba(217,119,6,0.12)",
    color: "var(--warning, #d97706)",
    fontSize: 11,
    fontWeight: 700,
  },
  list: {
    display: "flex",
    flexDirection: "column" as const,
  },
  item: {
    display: "flex",
    flexDirection: "column" as const,
  },
  itemToggle: {
    width: "100%",
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: 12,
    padding: "12px 14px",
    background: "transparent",
    border: "none",
    color: "var(--text-primary)",
    cursor: "pointer",
    textAlign: "left" as const,
  },
  itemTitleWrap: {
    display: "flex",
    alignItems: "center",
    gap: 8,
    minWidth: 0,
    flexWrap: "wrap" as const,
  },
  statusDot: {
    width: 7,
    height: 7,
    borderRadius: "50%",
    flexShrink: 0,
  },
  itemTitle: {
    fontSize: 12.5,
    fontWeight: 600,
    color: "var(--text-primary)",
    fontFamily: "var(--font-mono)",
    wordBreak: "break-word" as const,
  },
  modeBadge: {
    display: "inline-flex",
    alignItems: "center",
    padding: "2px 7px",
    borderRadius: 999,
    border: "1px solid var(--border-dim)",
    fontSize: 10.5,
    fontWeight: 700,
    lineHeight: 1.2,
    flexShrink: 0,
  },
  refBadge: {
    display: "inline-flex",
    alignItems: "center",
    padding: "2px 7px",
    borderRadius: 999,
    background: "color-mix(in srgb, var(--accent) 10%, transparent)",
    border: "1px solid color-mix(in srgb, var(--accent) 18%, transparent)",
    color: "var(--accent)",
    fontSize: 10.5,
    fontWeight: 700,
    lineHeight: 1.2,
  },
  subAgentBadge: {
    display: "inline-flex",
    alignItems: "center",
    padding: "2px 7px",
    borderRadius: 999,
    border: "1px solid var(--border-dim)",
    fontSize: 10.5,
    fontWeight: 700,
    lineHeight: 1.2,
    flexShrink: 0,
  },
  subAgentSpeed: {
    fontSize: 11.5,
    fontWeight: 700,
    color: "var(--accent)",
    fontFamily: "var(--font-mono)",
    flexShrink: 0,
  },
  itemRightWrap: {
    display: "flex",
    alignItems: "center",
    gap: 12,
    minWidth: 0,
    flex: 1,
    justifyContent: "flex-end",
  },
  itemDisplayText: {
    fontSize: 11.5,
    color: "var(--text-secondary)",
    whiteSpace: "nowrap",
    overflow: "hidden",
    textOverflow: "ellipsis",
    textAlign: "right" as const,
  },
  itemStatus: {
    fontSize: 11.5,
    fontWeight: 600,
    flexShrink: 0,
  },
  itemBody: {
    display: "flex",
    flexDirection: "column" as const,
    gap: 12,
    padding: "0 14px 14px",
  },
  block: {
    display: "flex",
    flexDirection: "column" as const,
    gap: 6,
  },
  blockLabel: {
    fontSize: 11,
    fontWeight: 700,
    color: "var(--text-secondary)",
    letterSpacing: 0.2,
  },
  blockCode: {
    margin: 0,
    padding: "10px 12px",
    borderRadius: 13,
    background: "color-mix(in srgb, var(--bg-input) 70%, transparent)",
    border: "1px solid var(--chat-glass-border)",
    color: "var(--text-primary)",
    fontSize: 12,
    lineHeight: 1.55,
    fontFamily: "var(--font-mono)",
    whiteSpace: "pre-wrap" as const,
    wordBreak: "break-word" as const,
    overflowX: "auto" as const,
  },
  blockText: {
    margin: 0,
    padding: "10px 12px",
    borderRadius: 13,
    background: "color-mix(in srgb, var(--bg-input) 70%, transparent)",
    border: "1px solid var(--chat-glass-border)",
    color: "var(--text-primary)",
    fontSize: 12.5,
    lineHeight: 1.6,
    whiteSpace: "pre-wrap" as const,
    wordBreak: "break-word" as const,
  },
  refList: {
    display: "flex",
    flexDirection: "column" as const,
    gap: 8,
  },
  refButton: {
    display: "flex",
    flexDirection: "column" as const,
    alignItems: "flex-start",
    gap: 4,
    width: "100%",
    padding: "10px 12px",
    borderRadius: 13,
    border: "1px solid var(--chat-glass-border)",
    background: "color-mix(in srgb, var(--bg-input) 74%, transparent)",
    cursor: "pointer",
    textAlign: "left" as const,
    color: "var(--text-primary)",
  },
  refButtonActive: {
    borderColor: "color-mix(in srgb, var(--accent) 36%, transparent)",
    background: "color-mix(in srgb, var(--accent) 8%, var(--bg-card))",
  },
  refButtonHeader: {
    display: "flex",
    alignItems: "center",
    gap: 6,
  },
  refButtonTitle: {
    fontSize: 12,
    fontWeight: 700,
  },
  refButtonMeta: {
    fontSize: 11,
    color: "var(--text-secondary)",
    fontFamily: "var(--font-mono)",
  },
  refButtonPreview: {
    fontSize: 11.5,
    color: "var(--text-secondary)",
    lineHeight: 1.5,
    whiteSpace: "pre-wrap" as const,
    wordBreak: "break-word" as const,
  },
  loadingState: {
    display: "flex",
    alignItems: "center",
    gap: 8,
    padding: "10px 12px",
    borderRadius: 12,
    background: "var(--bg-card)",
    border: "1px solid var(--border-dim)",
    color: "var(--text-secondary)",
    fontSize: 12,
  },
  loadingIcon: {
    animation: "spin 1s linear infinite",
  },
  errorState: {
    padding: "10px 12px",
    borderRadius: 12,
    background: "color-mix(in srgb, var(--danger) 8%, var(--bg-card))",
    border: "1px solid color-mix(in srgb, var(--danger) 20%, transparent)",
    color: "var(--danger)",
    fontSize: 12,
    lineHeight: 1.5,
    whiteSpace: "pre-wrap" as const,
    wordBreak: "break-word" as const,
  },
  infoState: {
    padding: "10px 12px",
    borderRadius: 12,
    background: "rgba(217,119,6,0.08)",
    border: "1px solid rgba(217,119,6,0.18)",
    color: "var(--text-secondary)",
    fontSize: 12,
    lineHeight: 1.5,
  },
  summaryContent: {
    padding: "10px 12px",
    borderRadius: 12,
    background: "var(--bg-card)",
    border: "1px solid var(--border-dim)",
    color: "var(--text-primary)",
    fontSize: 12.5,
    lineHeight: 1.6,
  },
  pendingText: {
    padding: "10px 12px",
    borderRadius: 12,
    background: "var(--bg-card)",
    border: "1px dashed var(--border-dim)",
    color: "var(--text-secondary)",
    fontSize: 12,
  },
} satisfies Record<string, CSSProperties>;

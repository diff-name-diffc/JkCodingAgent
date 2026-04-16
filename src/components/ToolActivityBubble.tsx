import { useMemo, useState } from "react";
import type { CSSProperties } from "react";
import { ChevronDown, ChevronRight, Wrench } from "lucide-react";
import type { DispatcherToolResultMode } from "../types";

export interface ToolActivityItem {
  key: string;
  name: string;
  input?: string;
  output?: string;
  resultMode?: DispatcherToolResultMode;
  status: "running" | "completed";
}

interface ToolActivityBubbleProps {
  tools: ToolActivityItem[];
  title?: string;
}

export function ToolActivityBubble({
  tools,
  title = "工具活动",
}: ToolActivityBubbleProps) {
  const [expandedTools, setExpandedTools] = useState<Record<string, boolean>>({});

  const runningCount = useMemo(
    () => tools.filter((tool) => tool.status === "running").length,
    [tools],
  );

  if (tools.length === 0) {
    return null;
  }

  const toggleTool = (key: string) => {
    setExpandedTools((prev) => ({ ...prev, [key]: !prev[key] }));
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
          {runningCount > 0 && <span style={styles.runningBadge}>运行中 {runningCount}</span>}
        </div>
      </div>
      <div style={styles.list}>
        {tools.map((tool, index) => {
          const expanded = expandedTools[tool.key] ?? false;
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
                        tool.status === "running" ? "var(--success)" : "var(--text-hint)",
                    }}
                  />
                  <span style={styles.itemTitle}>{tool.name}</span>
                  {tool.resultMode && (
                    <span style={toolModeBadgeStyle(tool.resultMode)}>
                      {toolResultModeLabel(tool.resultMode)}
                    </span>
                  )}
                </div>
                <span
                  style={{
                    ...styles.itemStatus,
                    color:
                      tool.status === "running" ? "var(--success)" : "var(--text-secondary)",
                  }}
                >
                  {tool.status === "running" ? "执行中" : "已完成"}
                </span>
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
                  {tool.output?.trim() ? (
                    <div style={styles.block}>
                      <div style={styles.blockLabel}>执行返回</div>
                      <pre className="session-selectable" style={styles.blockCode}>
                        {tool.output}
                      </pre>
                    </div>
                  ) : (
                    <div style={styles.pendingText}>等待工具返回...</div>
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

function toolResultModeLabel(mode: DispatcherToolResultMode): string {
  switch (mode) {
    case "summary":
      return "摘要";
    case "conservative_summary":
      return "保守压缩";
    case "raw":
    default:
      return "原文";
  }
}

function toolModeBadgeStyle(mode: DispatcherToolResultMode): CSSProperties {
  const accent =
    mode === "summary"
      ? "var(--accent)"
      : mode === "conservative_summary"
        ? "var(--warning, #d97706)"
        : "var(--text-hint)";
  const background =
    mode === "summary"
      ? "color-mix(in srgb, var(--accent) 12%, transparent)"
      : mode === "conservative_summary"
        ? "rgba(217,119,6,0.12)"
        : "var(--bg-hover)";

  return {
    ...styles.modeBadge,
    color: accent,
    background,
    borderColor: `color-mix(in srgb, ${accent} 24%, transparent)`,
  };
}

const styles = {
  container: {
    width: "100%",
    border: "1px solid var(--border-dim)",
    borderRadius: 16,
    background: "color-mix(in srgb, var(--bg-card) 84%, var(--bg-subtle))",
    boxShadow: "var(--shadow-xs)",
    overflow: "hidden",
  },
  header: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: 12,
    padding: "10px 14px",
    background: "color-mix(in srgb, var(--bg-subtle) 70%, transparent)",
    borderBottom: "1px solid var(--border-dim)",
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
    background: "var(--bg-card)",
    border: "1px solid var(--border-dim)",
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
  itemStatus: {
    flexShrink: 0,
    fontSize: 11.5,
    fontWeight: 600,
  },
  itemBody: {
    display: "flex",
    flexDirection: "column" as const,
    gap: 10,
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
    color: "var(--text-hint)",
    letterSpacing: 0.3,
    textTransform: "uppercase" as const,
  },
  blockCode: {
    margin: 0,
    padding: "10px 12px",
    borderRadius: 12,
    background: "var(--bg-root)",
    border: "1px solid var(--border-dim)",
    color: "var(--text-secondary)",
    fontSize: 11.5,
    lineHeight: 1.65,
    fontFamily: "var(--font-mono)",
    whiteSpace: "pre-wrap" as const,
    wordBreak: "break-word" as const,
    maxHeight: 260,
    overflow: "auto" as const,
  },
  pendingText: {
    fontSize: 11.5,
    color: "var(--text-hint)",
    fontStyle: "italic",
  },
};

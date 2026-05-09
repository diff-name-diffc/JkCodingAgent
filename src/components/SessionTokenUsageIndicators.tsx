import type { CSSProperties } from "react";
import type { DispatcherSessionTokenUsage } from "../types";

interface SessionTokenUsageIndicatorsProps {
  entries: DispatcherSessionTokenUsage[];
}

export function SessionTokenUsageIndicators({ entries }: SessionTokenUsageIndicatorsProps) {
  if (entries.length === 0) {
    return null;
  }

  return (
    <div style={styles.container}>
      {entries.map((entry) => {
        const percent =
          entry.contextWindowCapacity > 0
            ? Math.min(100, (entry.contextWindowTokens / entry.contextWindowCapacity) * 100)
            : 0;
        const sourceLabel = entry.sourceKind === "summary" ? "摘要" : "主模型";

        return (
          <div key={`${entry.model}-${entry.sourceKind}`} style={styles.item} title={entry.model}>
            <div style={styles.itemHeader}>
              <span style={styles.model}>{shortModelName(entry.model)}</span>
              <span style={styles.source}>{sourceLabel}</span>
            </div>
            <div style={styles.barTrack}>
              <div style={{ ...styles.barFill, width: `${percent}%` }} />
            </div>
            <div style={styles.meta}>
              <span>总 {formatTokenCount(entry.totalTokens)}</span>
              <span>上下文 {percent.toFixed(1)}%</span>
            </div>
          </div>
        );
      })}
    </div>
  );
}

function shortModelName(model: string): string {
  const trimmed = model.trim();
  if (trimmed.length <= 18) {
    return trimmed || "model";
  }
  return `${trimmed.slice(0, 8)}...${trimmed.slice(-7)}`;
}

function formatTokenCount(value: number): string {
  if (value >= 1_000_000) {
    return `${(value / 1_000_000).toFixed(2)}M`;
  }
  if (value >= 1_000) {
    return `${(value / 1_000).toFixed(1)}K`;
  }
  return String(value);
}

const styles = {
  container: {
    display: "flex",
    alignItems: "center",
    gap: 8,
    flexWrap: "wrap",
  },
  item: {
    minWidth: 116,
    padding: "6px 8px",
    border: "1px solid var(--border-dim)",
    borderRadius: 8,
    background: "color-mix(in srgb, var(--bg-card) 88%, transparent)",
    boxShadow: "var(--shadow-xs)",
  },
  itemHeader: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: 8,
    marginBottom: 5,
  },
  model: {
    minWidth: 0,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
    fontFamily: "var(--font-mono)",
    fontSize: 10.5,
    color: "var(--text-secondary)",
  },
  source: {
    flexShrink: 0,
    fontSize: 10,
    color: "var(--text-hint)",
  },
  barTrack: {
    height: 4,
    borderRadius: 999,
    background: "var(--bg-input)",
    overflow: "hidden",
  },
  barFill: {
    height: "100%",
    borderRadius: 999,
    background: "linear-gradient(90deg, var(--accent), var(--success, #34c759))",
  },
  meta: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: 8,
    marginTop: 4,
    fontFamily: "var(--font-mono)",
    fontSize: 10,
    color: "var(--text-hint)",
  },
} satisfies Record<string, CSSProperties>;

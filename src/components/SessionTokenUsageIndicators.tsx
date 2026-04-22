import { memo, useMemo, useState } from "react";
import type { CSSProperties } from "react";
import type { DispatcherSessionTokenUsage } from "../types";

interface SessionTokenUsageIndicatorsProps {
  entries: DispatcherSessionTokenUsage[];
}

function formatTokenCount(value: number): string {
  return new Intl.NumberFormat(undefined).format(value);
}

function formatUpdatedAt(value: string): string {
  if (!value.trim()) {
    return "尚未使用";
  }
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(date);
}

function summarizeModel(model: string): string {
  const parts = model.split(/[^a-zA-Z0-9]+/).filter(Boolean);
  if (parts.length >= 2) {
    return `${parts[0][0]}${parts[parts.length - 1][0]}`.toUpperCase();
  }
  return model.slice(0, 2).toUpperCase();
}

function usageTone(ratio: number): { color: string; shadow: string } {
  if (ratio >= 0.8) {
    return { color: "var(--usage-danger)", shadow: "rgba(239, 68, 68, 0.22)" };
  }
  if (ratio >= 0.5) {
    return { color: "var(--usage-warn)", shadow: "rgba(245, 158, 11, 0.22)" };
  }
  return { color: "var(--accent)", shadow: "color-mix(in srgb, var(--accent) 22%, transparent)" };
}

function ringStyle(entry: DispatcherSessionTokenUsage): CSSProperties {
  const ratio =
    entry.contextWindowCapacity > 0
      ? Math.min(entry.contextWindowTokens / entry.contextWindowCapacity, 1)
      : 0;
  const { color, shadow } = usageTone(ratio);
  const degrees = ratio <= 0 ? 0 : Math.max(8, Math.round(ratio * 360));

  return {
    ...styles.ring,
    color,
    boxShadow: `0 10px 22px ${shadow}`,
    background: `conic-gradient(${color} 0deg ${degrees}deg, color-mix(in srgb, var(--border-dim) 78%, transparent) ${degrees}deg 360deg)`,
  };
}

function sourceLabel(entry: DispatcherSessionTokenUsage): string {
  return entry.sourceKind === "summary" ? "摘要模型" : "主对话模型";
}

export const SessionTokenUsageIndicators = memo(function SessionTokenUsageIndicators({
  entries,
}: SessionTokenUsageIndicatorsProps) {
  const [hoveredModel, setHoveredModel] = useState<string | null>(null);
  const orderedEntries = useMemo(
    () =>
      [...entries].sort((a, b) => {
        if (a.updatedAt === b.updatedAt) {
          return a.model.localeCompare(b.model);
        }
        return a.updatedAt < b.updatedAt ? 1 : -1;
      }),
    [entries],
  );

  if (orderedEntries.length === 0) {
    return null;
  }

  return (
    <div style={styles.wrap}>
      {orderedEntries.map((entry) => {
        const ratio =
          entry.contextWindowCapacity > 0
            ? entry.contextWindowTokens / entry.contextWindowCapacity
            : 0;
        const percent = Math.min(Math.round(ratio * 100), 100);
        const isHovered = hoveredModel === entry.model;

        return (
          <div
            key={entry.model}
            style={styles.itemWrap}
            onMouseEnter={() => setHoveredModel(entry.model)}
            onMouseLeave={() =>
              setHoveredModel((current) => (current === entry.model ? null : current))
            }
          >
            <button
              type="button"
              style={styles.itemButton}
              onFocus={() => setHoveredModel(entry.model)}
              onBlur={() =>
                setHoveredModel((current) => (current === entry.model ? null : current))
              }
              title={`${entry.model}：窗口 ${formatTokenCount(entry.contextWindowTokens)} / ${formatTokenCount(entry.contextWindowCapacity)}`}
              aria-label={`${entry.model} 窗口 token 占用 ${percent}%`}
            >
              <span style={ringStyle(entry)}>
                <span style={styles.ringInner}>
                  <span style={styles.ringBadge}>{summarizeModel(entry.model)}</span>
                </span>
              </span>
            </button>
            {isHovered ? (
              <div style={styles.tooltip}>
                <div style={styles.tooltipTitle}>{entry.model}</div>
                <div style={styles.tooltipLine}>
                  <span>窗口占用</span>
                  <strong>
                    {formatTokenCount(entry.contextWindowTokens)} /{" "}
                    {formatTokenCount(entry.contextWindowCapacity)}
                  </strong>
                </div>
                <div style={styles.tooltipLine}>
                  <span>输入</span>
                  <strong>{formatTokenCount(entry.promptTokens)}</strong>
                </div>
                <div style={styles.tooltipLine}>
                  <span>输出</span>
                  <strong>{formatTokenCount(entry.completionTokens)}</strong>
                </div>
                <div style={styles.tooltipLine}>
                  <span>总计</span>
                  <strong>{formatTokenCount(entry.totalTokens)}</strong>
                </div>
                {entry.cachedTokens > 0 ? (
                  <div style={styles.tooltipLine}>
                    <span>缓存命中</span>
                    <strong>{formatTokenCount(entry.cachedTokens)}</strong>
                  </div>
                ) : null}
                <div style={styles.tooltipMeta}>
                  <span>{sourceLabel(entry)}</span>
                  <span>{formatUpdatedAt(entry.updatedAt)}</span>
                </div>
              </div>
            ) : null}
          </div>
        );
      })}
    </div>
  );
});

const styles = {
  wrap: {
    display: "flex",
    alignItems: "center",
    gap: "8px",
    flexWrap: "wrap" as const,
  },
  itemWrap: {
    position: "relative" as const,
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
  },
  itemButton: {
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    padding: 0,
    border: "none",
    background: "transparent",
    cursor: "default",
  },
  ring: {
    width: "34px",
    height: "34px",
    borderRadius: "999px",
    padding: "3px",
    transition: "transform 140ms ease, box-shadow 140ms ease",
  },
  ringInner: {
    width: "100%",
    height: "100%",
    borderRadius: "999px",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    background:
      "radial-gradient(circle at top, color-mix(in srgb, var(--bg-card) 96%, white 4%), color-mix(in srgb, var(--bg-panel) 90%, transparent))",
    border: "1px solid color-mix(in srgb, var(--border-dim) 72%, transparent)",
  },
  ringBadge: {
    fontSize: "10px",
    fontWeight: 800,
    letterSpacing: "0.04em",
    color: "var(--text-secondary)",
  },
  tooltip: {
    position: "absolute" as const,
    right: 0,
    bottom: "calc(100% + 10px)",
    minWidth: "220px",
    padding: "10px 12px",
    borderRadius: "14px",
    border: "1px solid color-mix(in srgb, var(--accent) 18%, var(--border-dim))",
    background:
      "linear-gradient(180deg, color-mix(in srgb, var(--bg-card) 98%, transparent), color-mix(in srgb, var(--bg-panel) 94%, transparent))",
    boxShadow: "0 20px 50px rgba(15, 23, 42, 0.16)",
    backdropFilter: "blur(16px)",
    WebkitBackdropFilter: "blur(16px)",
    zIndex: 20,
    pointerEvents: "none" as const,
  },
  tooltipTitle: {
    fontSize: "12px",
    fontWeight: 700,
    color: "var(--text-primary)",
    marginBottom: "8px",
  },
  tooltipLine: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: "12px",
    fontSize: "11px",
    color: "var(--text-secondary)",
    marginBottom: "4px",
  },
  tooltipMeta: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: "10px",
    marginTop: "8px",
    paddingTop: "8px",
    borderTop: "1px solid color-mix(in srgb, var(--border-dim) 78%, transparent)",
    fontSize: "10px",
    color: "var(--text-hint)",
  },
} satisfies Record<string, CSSProperties>;

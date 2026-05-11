import { useEffect, useRef, useState, type CSSProperties } from "react";
import * as Popover from "@radix-ui/react-popover";
import type { DispatcherSessionTokenUsage } from "../types";
import { formatTokenCount } from "../utils";

interface SessionTokenUsageIndicatorsProps {
  entries: DispatcherSessionTokenUsage[];
}

export function SessionTokenUsageIndicators({ entries }: SessionTokenUsageIndicatorsProps) {
  const [hoveredKey, setHoveredKey] = useState<string | null>(null);
  const closeTimerRef = useRef<number | null>(null);

  useEffect(() => {
    return () => {
      if (closeTimerRef.current !== null) {
        window.clearTimeout(closeTimerRef.current);
      }
    };
  }, []);

  function clearCloseTimer() {
    if (closeTimerRef.current !== null) {
      window.clearTimeout(closeTimerRef.current);
      closeTimerRef.current = null;
    }
  }

  function openIndicator(itemKey: string) {
    clearCloseTimer();
    setHoveredKey(itemKey);
  }

  function scheduleClose(itemKey: string, delay = 180) {
    clearCloseTimer();
    closeTimerRef.current = window.setTimeout(() => {
      setHoveredKey((current) => (current === itemKey ? null : current));
      closeTimerRef.current = null;
    }, delay);
  }

  if (entries.length === 0) {
    return null;
  }

  return (
    <div style={styles.container}>
      {entries.map((entry) => {
        const itemKey = `${entry.model}-${entry.sourceKind}`;
        const percent =
          entry.contextWindowCapacity > 0
            ? Math.min(100, (entry.contextWindowTokens / entry.contextWindowCapacity) * 100)
            : 0;
        const sourceLabel = entry.sourceKind === "summary" ? "摘要" : "主模型";

        return (
          <Popover.Root key={itemKey} open={hoveredKey === itemKey}>
            <Popover.Trigger asChild>
              <button
                type="button"
                style={styles.item}
                aria-label={`${sourceLabel} ${entry.model} 上下文占用 ${percent.toFixed(1)}%`}
                onFocus={() => openIndicator(itemKey)}
                onBlur={() => scheduleClose(itemKey, 80)}
                onPointerEnter={() => openIndicator(itemKey)}
                onPointerLeave={() => scheduleClose(itemKey)}
              >
                <TokenUsageRing percent={percent} />
              </button>
            </Popover.Trigger>
            <Popover.Portal>
              <Popover.Content
                side="top"
                align="end"
                sideOffset={10}
                collisionPadding={12}
                style={styles.tooltip}
                onOpenAutoFocus={(event) => event.preventDefault()}
                onCloseAutoFocus={(event) => event.preventDefault()}
                onPointerEnter={() => openIndicator(itemKey)}
                onPointerLeave={() => scheduleClose(itemKey)}
              >
                <div style={styles.tooltipHeader}>
                  <span style={styles.model}>{shortModelName(entry.model)}</span>
                  <span style={styles.source}>{sourceLabel}</span>
                </div>
                <div style={styles.percentRow}>
                  <span>上下文占用</span>
                  <strong>{percent.toFixed(1)}%</strong>
                </div>
                <div style={styles.detailGrid}>
                  <Detail label="窗口" value={formatTokenCount(entry.contextWindowCapacity)} />
                  <Detail label="已占" value={formatTokenCount(entry.contextWindowTokens)} />
                  <Detail label="输入" value={formatTokenCount(entry.promptTokens)} />
                  <Detail label="输出" value={formatTokenCount(entry.completionTokens)} />
                  <Detail label="缓存" value={formatTokenCount(entry.cachedTokens)} />
                  <Detail label="总计" value={formatTokenCount(entry.totalTokens)} />
                </div>
              </Popover.Content>
            </Popover.Portal>
          </Popover.Root>
        );
      })}
    </div>
  );
}

function TokenUsageRing({ percent }: { percent: number }) {
  const radius = 10;
  const circumference = 2 * Math.PI * radius;
  const clamped = Math.max(0, Math.min(100, percent));
  const dashOffset = circumference * (1 - clamped / 100);

  return (
    <svg viewBox="0 0 28 28" width="28" height="28" style={styles.ringSvg} aria-hidden="true">
      <circle cx="14" cy="14" r={radius} style={styles.ringTrack} />
      <circle
        cx="14"
        cy="14"
        r={radius}
        style={{
          ...styles.ringProgress,
          strokeDasharray: circumference,
          strokeDashoffset: dashOffset,
        }}
      />
    </svg>
  );
}

function Detail({ label, value }: { label: string; value: string }) {
  return (
    <div style={styles.detailItem}>
      <span>{label}</span>
      <strong>{value}</strong>
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

const styles = {
  container: {
    display: "flex",
    alignItems: "center",
    gap: 6,
    flexWrap: "wrap",
  },
  item: {
    width: 28,
    height: 28,
    padding: 0,
    border: "none",
    borderRadius: 999,
    background: "transparent",
    outline: "none",
    cursor: "default",
    flexShrink: 0,
  },
  ringSvg: {
    display: "block",
    overflow: "visible",
  },
  ringTrack: {
    fill: "none",
    stroke: "color-mix(in srgb, var(--text-muted) 18%, transparent)",
    strokeWidth: 4,
  },
  ringProgress: {
    fill: "none",
    stroke: "color-mix(in srgb, var(--text-muted) 78%, transparent)",
    strokeWidth: 4,
    strokeLinecap: "round",
    transform: "rotate(-90deg)",
    transformOrigin: "14px 14px",
    transition: "stroke-dashoffset 420ms ease",
  },
  model: {
    minWidth: 0,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
    fontFamily: "var(--font-mono)",
    fontSize: 11,
    color: "var(--text-primary)",
  },
  source: {
    flexShrink: 0,
    fontSize: 10,
    fontWeight: 700,
    color: "var(--text-muted)",
  },
  tooltip: {
    width: 236,
    padding: "10px 11px",
    borderRadius: 12,
    border: "1px solid var(--border-medium)",
    background: "var(--bg-card)",
    boxShadow: "0 18px 48px rgba(0,0,0,0.24)",
    zIndex: 9999,
    pointerEvents: "auto",
  },
  tooltipHeader: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: 8,
    marginBottom: 8,
  },
  percentRow: {
    display: "flex",
    justifyContent: "space-between",
    alignItems: "baseline",
    gap: 10,
    padding: "7px 8px",
    borderRadius: 8,
    background: "var(--bg-subtle)",
    color: "var(--text-secondary)",
    fontSize: 11.5,
    marginBottom: 8,
  },
  detailGrid: {
    display: "grid",
    gridTemplateColumns: "1fr 1fr",
    gap: "6px 10px",
  },
  detailItem: {
    display: "flex",
    justifyContent: "space-between",
    gap: 8,
    color: "var(--text-muted)",
    fontSize: 11,
    fontFamily: "var(--font-mono)",
  },
} satisfies Record<string, CSSProperties>;

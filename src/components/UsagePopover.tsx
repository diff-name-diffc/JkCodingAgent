import { useMemo, useState } from "react";
import * as Popover from "@radix-ui/react-popover";
import { Activity } from "lucide-react";
import type { ClaudeUsageData, CodexUsageData, UsageSource, UsageWindow } from "../types";
import { useUsageSnapshot } from "../hooks/useUsageSnapshot";
import { getUsageColor } from "../utils";

function formatResetTime(resetAt?: number | null): string | null {
  if (!resetAt) return null;
  const date = new Date(resetAt * 1000);
  if (Number.isNaN(date.getTime())) return null;
  return new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(date);
}

function UsageMetricRow({ label, window }: { label: string; window: UsageWindow }) {
  const color = getUsageColor(window.remainingPercent);
  const resetLabel = formatResetTime(window.resetAt);

  return (
    <div className="ai-usage-metric-row">
      <span className="ai-usage-metric-label">{label}</span>
      <span className="ai-usage-metric-value" style={{ color }}>剩余 {window.remainingPercent}%</span>
      {resetLabel && <span className="ai-usage-metric-meta">{resetLabel}</span>}
    </div>
  );
}

function SourceCard<T>({
  title,
  subtitle,
  source,
  metrics,
}: {
  title: string;
  subtitle?: string | null;
  source: UsageSource<T>;
  metrics: Array<{ label: string; window?: UsageWindow | null }>;
}) {
  return (
    <section className="ai-usage-source-section">
      <div className="ai-usage-source-head">
        <div className="ai-usage-source-title">{title}</div>
        {subtitle ? <div className="ai-usage-source-subtitle">{subtitle}</div> : null}
      </div>

      {source.status === "unavailable" ? (
        <div className="ai-usage-status-text">{source.reason}</div>
      ) : (
        <div className="ai-usage-metric-list">
          {metrics.some((metric) => metric.window) ? (
            metrics.map((metric) =>
              metric.window ? (
                <UsageMetricRow key={metric.label} label={metric.label} window={metric.window} />
              ) : null,
            )
          ) : (
            <div className="ai-usage-status-text">暂未返回用量窗口数据。</div>
          )}
        </div>
      )}
    </section>
  );
}

function codexSubtitle(source: UsageSource<CodexUsageData>): string | null {
  if (source.status !== "available") return null;
  const parts = [source.data.planType, source.data.email].filter(Boolean);
  return parts.length > 0 ? parts.join(" · ") : null;
}

export function UsagePopover() {
  const [open, setOpen] = useState(false);
  const { snapshot, loading, error } = useUsageSnapshot(open);

  const claudeMetrics = useMemo(
    () => [
      {
        label: "5 小时",
        window: snapshot?.claude.status === "available" ? snapshot.claude.data.fiveHour : null,
      },
      {
        label: "7 天",
        window: snapshot?.claude.status === "available" ? snapshot.claude.data.sevenDay : null,
      },
    ],
    [snapshot],
  );

  const codexMetrics = useMemo(
    () => [
      {
        label: "5 小时",
        window: snapshot?.codex.status === "available" ? snapshot.codex.data.primary : null,
      },
      {
        label: "7 天",
        window: snapshot?.codex.status === "available" ? snapshot.codex.data.secondary : null,
      },
    ],
    [snapshot],
  );

  return (
      <Popover.Root open={open} onOpenChange={setOpen}>
        <Popover.Trigger asChild>
        <button className="ai-sidebar-tool-button" title="用量">
          <Activity size={14} strokeWidth={1.8} color="var(--text-hint)" />
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content side="top" align="start" sideOffset={8} className="ai-usage-popover ai-migrated-usage-popover">
          <div className="ai-usage-popover-header">
            <div className="ai-usage-popover-title">用量</div>
          </div>

          {loading ? (
            <div className="ai-usage-status-text">正在加载用量…</div>
          ) : error ? (
            <div className="ai-usage-status-text is-error">加载用量失败：{error}</div>
          ) : snapshot ? (
            <div className="ai-usage-source-list">
              <SourceCard<ClaudeUsageData>
                title="Claude Code"
                source={snapshot.claude}
                metrics={claudeMetrics}
              />
              <SourceCard<CodexUsageData>
                title="Codex"
                subtitle={codexSubtitle(snapshot.codex)}
                source={snapshot.codex}
                metrics={codexMetrics}
              />
            </div>
          ) : (
            <div className="ai-usage-status-text">暂时没有用量数据。</div>
          )}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

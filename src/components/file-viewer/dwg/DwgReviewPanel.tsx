import type { CSSProperties } from "react";
import type { CadReviewRun, CadReviewRunDetail, DwgParseSummary } from "../../../types";

const tokenStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  padding: "5px 8px",
  borderRadius: 999,
  background: "color-mix(in srgb, var(--bg-panel) 92%, transparent)",
  border: "1px solid var(--border-dim)",
  color: "var(--text-secondary)",
  fontSize: 11.5,
  fontWeight: 600,
};

export function DwgReviewPanel({
  summary,
  reviewRuns,
  reviewDetail,
  activeReviewRunId,
  activeIssueId,
  onActiveReviewRunChange,
  onActiveIssueChange,
}: {
  summary: DwgParseSummary | null;
  reviewRuns: CadReviewRun[];
  reviewDetail: CadReviewRunDetail | null;
  activeReviewRunId: string | null;
  activeIssueId: string | null;
  onActiveReviewRunChange: (runId: string | null) => void;
  onActiveIssueChange: (issueId: string | null) => void;
}) {
  return (
    <>
      <PanelCard title="解析摘要">
        {summary ? (
          <div style={{ display: "grid", gap: 10 }}>
            <MetaRow label="实体总数" value={String(summary.totalEntities)} />
            <MetaRow label="未知实体" value={String(summary.unknownEntityCount)} />
            <MetaRow label="图层数" value={String(summary.layers.length)} />
            {summary.bounds && (
              <MetaRow
                label="范围"
                value={`${summary.bounds.minX.toFixed(1)}, ${summary.bounds.minY.toFixed(1)} → ${summary.bounds.maxX.toFixed(1)}, ${summary.bounds.maxY.toFixed(1)}`}
              />
            )}
            <div style={{ display: "flex", flexWrap: "wrap", gap: 6 }}>
              {summary.layers.slice(0, 10).map((layer) => (
                <span key={layer.name} style={tokenStyle}>
                  {layer.name} · {layer.entityCount}
                </span>
              ))}
            </div>
          </div>
        ) : (
          <EmptyHint>还没有可展示的 DWG 摘要。</EmptyHint>
        )}
      </PanelCard>

      <PanelCard title="问题清单">
        {reviewRuns.length === 0 ? (
          <EmptyHint>当前会话还没有这个 DWG 的审查结果。</EmptyHint>
        ) : (
          <div style={{ display: "grid", gap: 12 }}>
            <div style={{ display: "grid", gap: 8 }}>
              {reviewRuns.map((run) => {
                const active = run.id === activeReviewRunId;
                return (
                  <button
                    key={run.id}
                    type="button"
                    onClick={() => onActiveReviewRunChange(run.id)}
                    style={{
                      textAlign: "left",
                      padding: 10,
                      borderRadius: 14,
                      border: active ? "1px solid var(--accent)" : "1px solid var(--border-dim)",
                      background: active ? "var(--accent-subtle)" : "transparent",
                      cursor: "pointer",
                    }}
                  >
                    <div style={{ fontSize: 13, fontWeight: 700, color: "var(--text-primary)" }}>
                      {run.summary}
                    </div>
                    <div style={{ fontSize: 11.5, color: "var(--text-muted)", marginTop: 4 }}>
                      {run.issueCount} 条问题 · {new Date(run.createdAt).toLocaleString()}
                    </div>
                  </button>
                );
              })}
            </div>

            {reviewDetail && (
              <div style={{ display: "grid", gap: 8 }}>
                {reviewDetail.issues.map((issue) => {
                  const active = issue.id === activeIssueId;
                  return (
                    <button
                      key={issue.id}
                      type="button"
                      onClick={() => onActiveIssueChange(issue.id)}
                      style={{
                        textAlign: "left",
                        padding: 12,
                        borderRadius: 14,
                        border: active ? "1px solid var(--accent)" : "1px solid var(--border-dim)",
                        background: active
                          ? "color-mix(in srgb, var(--accent) 10%, var(--bg-card))"
                          : "color-mix(in srgb, var(--bg-card) 90%, transparent)",
                        cursor: "pointer",
                      }}
                    >
                      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                        <span style={severityPill(issue.severity)}>{issue.severity}</span>
                        <span style={{ fontSize: 13, fontWeight: 700, color: "var(--text-primary)" }}>
                          {issue.title}
                        </span>
                      </div>
                      <div
                        style={{
                          marginTop: 6,
                          fontSize: 12,
                          color: "var(--text-secondary)",
                          lineHeight: 1.5,
                        }}
                      >
                        {issue.description}
                      </div>
                      {(issue.anchorPoint || issue.bbox) && (
                        <div
                          style={{
                            marginTop: 8,
                            fontSize: 11.5,
                            color: "var(--text-muted)",
                            fontFamily: "var(--font-mono)",
                          }}
                        >
                          {issue.anchorPoint
                            ? `定位: ${issue.anchorPoint.x.toFixed(2)}, ${issue.anchorPoint.y.toFixed(2)}`
                            : `范围: ${issue.bbox?.minX.toFixed(2)}, ${issue.bbox?.minY.toFixed(2)} → ${issue.bbox?.maxX.toFixed(2)}, ${issue.bbox?.maxY.toFixed(2)}`}
                        </div>
                      )}
                    </button>
                  );
                })}
              </div>
            )}
          </div>
        )}
      </PanelCard>
    </>
  );
}

function PanelCard({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section
      style={{
        borderRadius: 20,
        border: "1px solid var(--border-dim)",
        background: "color-mix(in srgb, var(--bg-card) 92%, transparent)",
        padding: 14,
        display: "grid",
        gap: 12,
      }}
    >
      <div
        style={{
          fontSize: 12,
          fontWeight: 700,
          letterSpacing: "0.08em",
          color: "var(--text-hint)",
        }}
      >
        {title}
      </div>
      {children}
    </section>
  );
}

function MetaRow({ label, value }: { label: string; value: string }) {
  return (
    <div style={{ display: "flex", justifyContent: "space-between", gap: 12, fontSize: 12.5 }}>
      <span style={{ color: "var(--text-muted)" }}>{label}</span>
      <span style={{ color: "var(--text-primary)", fontWeight: 600, textAlign: "right" }}>
        {value}
      </span>
    </div>
  );
}

function EmptyHint({ children }: { children: React.ReactNode }) {
  return <div style={{ fontSize: 12.5, color: "var(--text-muted)", lineHeight: 1.5 }}>{children}</div>;
}

function severityPill(severity: string): CSSProperties {
  const lower = severity.toLowerCase();
  const accent =
    lower === "high" || lower === "error"
      ? "#ef4444"
      : lower === "medium" || lower === "warning"
        ? "#f59e0b"
        : "#22c55e";
  return {
    display: "inline-flex",
    alignItems: "center",
    padding: "3px 8px",
    borderRadius: 999,
    background: `${accent}1A`,
    color: accent,
    fontSize: 11,
    fontWeight: 700,
    textTransform: "uppercase",
  };
}

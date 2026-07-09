import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Project } from "../types";
import { AiEmptyState, AiPanel, AiSectionHeader } from "./ui/sci-fi-shell";

interface DayStats {
  date: string;
  task_count: number;
  done_count: number;
  token_count: number;
}

interface ProjectAnalytics {
  project_id: string;
  project_name: string;
  task_count: number;
  done_count: number;
  token_count: number;
  tool_calls: number;
}

interface WeeklyAnalytics {
  daily: DayStats[];
  total_tasks: number;
  done_tasks: number;
  failed_tasks: number;
  total_input_tokens: number;
  total_output_tokens: number;
  total_tool_calls: number;
  total_duration_secs: number;
  claude_tasks: number;
  codex_tasks: number;
  projects: ProjectAnalytics[];
}

const DAY_LABELS = ["日", "一", "二", "三", "四", "五", "六"];

function heatmapColor(count: number): string {
  if (count === 0) return "var(--bg-card)";
  if (count === 1) return "color-mix(in srgb, var(--accent) 22%, var(--bg-card))";
  if (count <= 3) return "color-mix(in srgb, var(--accent) 44%, var(--bg-card))";
  if (count <= 6) return "color-mix(in srgb, var(--accent) 68%, var(--bg-card))";
  return "var(--accent)";
}

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}

function formatDuration(secs: number): string {
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m`;
  return `${Math.round(secs)}s`;
}

function getDayLabel(dateStr: string): string {
  const d = new Date(dateStr + "T00:00:00");
  return DAY_LABELS[d.getDay()];
}

function isToday(dateStr: string): boolean {
  return dateStr === new Date().toISOString().slice(0, 10);
}

function StatCard({ value, label }: { value: string; label: string }) {
  return (
    <AiPanel className="ai-stat-card">
      <div className="ai-stat-value">{value}</div>
      <div className="ai-stat-label">{label}</div>
    </AiPanel>
  );
}

export function AnalyticsDashboard({ projects: _projects }: { projects: Project[] }) {
  const [data, setData] = useState<WeeklyAnalytics | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<WeeklyAnalytics>("get_weekly_analytics")
      .then((d) => {
        setData(d);
        setLoading(false);
      })
      .catch((e) => {
        setError(String(e));
        setLoading(false);
      });
  }, []);

  if (loading) {
    return (
      <div className="ai-home-pane ai-analytics-pane">
        <AiEmptyState title="加载中..." />
      </div>
    );
  }

  if (error || !data) {
    return (
      <div className="ai-home-pane ai-analytics-pane">
        <AiEmptyState title={error ?? "暂无数据"} />
      </div>
    );
  }

  const totalTokens = data.total_input_tokens + data.total_output_tokens;
  const successRate =
    data.total_tasks > 0 ? Math.round((data.done_tasks / data.total_tasks) * 100) : 0;
  const totalAgents = data.claude_tasks + data.codex_tasks;
  const claudePct = totalAgents > 0 ? Math.round((data.claude_tasks / totalAgents) * 100) : 0;
  const codexPct = totalAgents > 0 ? 100 - claudePct : 0;

  return (
    <div className="ai-home-pane ai-analytics-pane">
      <AiSectionHeader title="最近 7 天" caption="编码活动概览" />

      <div className="ai-analytics-hero">
        <AiPanel className="ai-analytics-card">
          <div className="ai-analytics-title">任务热力</div>
          <div className="ai-heatmap-row">
            {data.daily.map((day) => {
              const today = isToday(day.date);
              const active = day.task_count > 0;
              return (
                <div key={day.date} className="ai-heatmap-day">
                  <div
                    className={`ai-heatmap-cell${today ? " is-today" : ""}`}
                    style={{ background: heatmapColor(day.task_count) }}
                    title={`${day.date}：${day.task_count} 个任务`}
                  />
                  <div className={`ai-heatmap-label${today ? " is-active" : ""}`}>
                    {getDayLabel(day.date)}
                  </div>
                  <div className={`ai-heatmap-label${active ? " is-active" : ""}`}>
                    {active ? day.task_count : "·"}
                  </div>
                </div>
              );
            })}
          </div>
          <div className="ai-heatmap-legend">
            <span>少</span>
            {[0, 1, 3, 5, 8].map((n) => (
              <span
                key={n}
                className="ai-heatmap-legend-swatch"
                style={{ background: heatmapColor(n) }}
              />
            ))}
            <span>多</span>
          </div>
        </AiPanel>

        <AiPanel className="ai-analytics-card">
          <div className="ai-analytics-title">运行态势</div>
          <div className="ai-agent-bars">
            <div className="ai-agent-meta">
              <span>失败任务</span>
              <span>{data.failed_tasks}</span>
            </div>
            <div className="ai-agent-track">
              <div
                className="ai-agent-fill"
                style={{
                  width: `${data.total_tasks > 0 ? Math.round((data.failed_tasks / data.total_tasks) * 100) : 0}%`,
                  background: "var(--danger)",
                  color: "var(--danger)",
                }}
              />
            </div>
            <div className="ai-agent-meta">
              <span>总耗时</span>
              <span>{formatDuration(data.total_duration_secs)}</span>
            </div>
          </div>
        </AiPanel>
      </div>

      <div className="ai-stat-grid">
        <StatCard value={String(data.total_tasks)} label="任务总数" />
        <StatCard value={`${successRate}%`} label="成功率" />
        <StatCard value={formatTokens(totalTokens)} label="总 Token" />
        <StatCard value={String(data.total_tool_calls)} label="工具调用" />
      </div>

      <div className="ai-analytics-row">
        <AiPanel className="ai-analytics-card">
          <div className="ai-analytics-title">智能体分布</div>
          {totalAgents === 0 ? (
            <AiEmptyState title="暂无数据" />
          ) : (
            <div className="ai-agent-bars">
              <div>
                <div className="ai-agent-meta">
                  <span>Claude Code</span>
                  <span>
                    {data.claude_tasks} 个任务（{claudePct}%）
                  </span>
                </div>
                <div className="ai-agent-track">
                  <div
                    className="ai-agent-fill"
                    style={{
                      width: `${claudePct}%`,
                      background: "var(--accent)",
                      color: "var(--accent)",
                    }}
                  />
                </div>
              </div>
              <div>
                <div className="ai-agent-meta">
                  <span>Codex</span>
                  <span>
                    {data.codex_tasks} 个任务（{codexPct}%）
                  </span>
                </div>
                <div className="ai-agent-track">
                  <div
                    className="ai-agent-fill"
                    style={{ width: `${codexPct}%`, background: "var(--warning)", color: "var(--warning)" }}
                  />
                </div>
              </div>
              {data.total_duration_secs > 0 && (
                <div className="ai-agent-meta">
                  总耗时：
                  <span>{formatDuration(data.total_duration_secs)}</span>
                </div>
              )}
            </div>
          )}
        </AiPanel>

        <AiPanel className="ai-analytics-card">
          <div className="ai-analytics-title">项目排行</div>
          {data.projects.length === 0 ? (
            <AiEmptyState title="暂无数据" />
          ) : (
            <div className="ai-project-rank-list">
              {data.projects.slice(0, 5).map((p, i) => {
                const ratio = Math.round(
                  (p.task_count / (data.projects[0]?.task_count || 1)) * 100,
                );
                return (
                  <div key={p.project_id} className="ai-project-rank-row">
                    <div className="ai-project-rank-fill" style={{ width: `${ratio}%` }} />
                    <span className="ai-project-rank-index">{i + 1}</span>
                    <span className="ai-project-rank-name">{p.project_name}</span>
                    <span className="ai-project-rank-count">{p.task_count}</span>
                  </div>
                );
              })}
            </div>
          )}
        </AiPanel>
      </div>
    </div>
  );
}

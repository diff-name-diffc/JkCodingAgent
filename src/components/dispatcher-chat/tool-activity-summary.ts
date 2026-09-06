/**
 * 工具活动语义聚合（UI-12）：把连续工具调用收成一行摘要。
 *
 * 摘要**只由结构化事实生成**（工具名、状态、耗时——02-design §5.2），
 * 不解析、不猜测工具输出内容；聚合结果可由同一 items 复算（验收条款）。
 * 失败/等待/运行中计数单列，由调用方以 StatusPill 双编码露出，
 * 保证「折叠不隐藏错误」。
 */
import type { ToolActivityItem } from "./tool-activity";

export type ToolVerbCategory =
  | "read"
  | "write"
  | "command"
  | "search"
  | "subagent"
  | "browser"
  | "image"
  | "graph"
  | "program"
  | "mcp"
  | "other";

/** 名字→动词类别。全集来自后端工具注册表（agent/tools/spec.rs 策略表）。 */
const TOOL_VERB_MAP: Record<string, ToolVerbCategory> = {
  read_file: "read",
  list_dir: "read",
  write_file: "write",
  edit_file: "write",
  exec: "command",
  local_zsh: "command",
  ssh_exec: "command",
  grep: "search",
  glob: "search",
  call_sub_agent: "subagent",
  submit_graph: "graph",
  run_tool_program: "program",
  generate_image: "image",
  edit_image: "image",
  analyze_image: "image",
  fetch_image: "image",
};

export function categorizeTool(name: string): ToolVerbCategory {
  if (name.startsWith("mcp__")) return "mcp";
  if (name.startsWith("browser_")) return "browser";
  return TOOL_VERB_MAP[name] ?? "other";
}

export interface ToolActivitySummary {
  total: number;
  counts: Partial<Record<ToolVerbCategory, number>>;
  failed: number;
  running: number;
  /** planned（等待执行）单列，不计入 running。 */
  planned: number;
  totalDurationMs: number;
}

export function summarizeToolActivity(
  items: readonly ToolActivityItem[],
): ToolActivitySummary {
  const counts: Partial<Record<ToolVerbCategory, number>> = {};
  let failed = 0;
  let running = 0;
  let planned = 0;
  let totalDurationMs = 0;
  for (const item of items) {
    const category = categorizeTool(item.name);
    counts[category] = (counts[category] ?? 0) + 1;
    if (item.status === "error") failed += 1;
    else if (item.planned) planned += 1;
    else if (item.status === "running") running += 1;
    totalDurationMs += item.durationMs ?? 0;
  }
  return { total: items.length, counts, failed, running, planned, totalDurationMs };
}

/** 类别文案按固定顺序拼接，保证同输入同输出（可复算/可快照）。 */
const CATEGORY_ORDER: readonly ToolVerbCategory[] = [
  "read",
  "write",
  "command",
  "search",
  "subagent",
  "browser",
  "image",
  "graph",
  "program",
  "mcp",
  "other",
];

const CATEGORY_TEXT: Record<ToolVerbCategory, (count: number) => string> = {
  read: (n) => `读取 ${n} 个文件`,
  write: (n) => `修改 ${n} 个文件`,
  command: (n) => `命令 ${n}`,
  search: (n) => `搜索 ${n}`,
  subagent: (n) => `子智能体 ${n}`,
  browser: (n) => `浏览器 ${n}`,
  image: (n) => `图像 ${n}`,
  graph: (n) => `执行图 ${n}`,
  program: (n) => `程序 ${n}`,
  mcp: (n) => `MCP ${n}`,
  other: (n) => `其他 ${n}`,
};

export function formatSummaryDuration(durationMs: number): string {
  if (durationMs < 1000) return `${Math.round(durationMs)}ms`;
  return `${(durationMs / 1000).toFixed(1)}s`;
}

/** 主文案：类别计数 + 总耗时。状态计数（失败/等待/运行中）由 StatusPill 承担，不重复进文本。 */
export function formatToolActivitySummary(summary: ToolActivitySummary): string {
  if (summary.total === 0) return "";
  const parts: string[] = [];
  for (const category of CATEGORY_ORDER) {
    const count = summary.counts[category];
    if (count && count > 0) parts.push(CATEGORY_TEXT[category](count));
  }
  parts.push(`总耗时 ${formatSummaryDuration(summary.totalDurationMs)}`);
  return parts.join(" · ");
}

import type {
  DispatcherToolArtifactRef,
  DispatcherToolResultMode,
  DispatcherToolRunRecord,
} from "../../types";

export type ToolCallStatus = "running" | "success" | "error";

export interface ToolCallItem {
  id: string;
  name: string;
  status: ToolCallStatus;
  durationMs?: number;
  input?: unknown;
  output?: unknown;
  errorText?: string;
}

/**
 * 工具调用活动在聊天消息流中的展示模型（「单一活动态实现」）：
 * 实时管道（live-tool-activity 的流式事件归并）与历史投影
 * （dispatcherChatView 的 buildDispatcherDisplayItems）共用本模块的
 * upsert / 错误判定 / 参数美化 / 运行树归并助手，保证两套卡片同口径；
 * 摘要聚合（summarizeToolActivity）也在此处，供 tool-call-card 折叠展示。
 */
export interface ToolActivityItem extends ToolCallItem {
  /** 所属会话；历史卡片展开时用它按需恢复运行树。 */
  workspaceId?: string;
  /** 外层模型工具调用对应的根运行记录 ID。 */
  runId?: string;
  /** 根运行及其内部步骤的扁平快照，展示层按 parentRunId 构造树。 */
  toolRuns?: DispatcherToolRunRecord[];
  detailRefs?: DispatcherToolArtifactRef[];
  resultMode?: DispatcherToolResultMode;
  /**
   * 已规划、尚未开始执行（UI-12：「等待」独立于「执行中」表达）。
   * 仅实时管道产生：toolPlanned 置 true，toolStarted/toolFinished 清除；
   * 历史投影皆为终态，天然不带此标记。
   */
  planned?: boolean;
  /** 仅用于计算流式工具调用耗时，不属于 ToolCallCard 的展示契约。 */
  startedAtMs?: number;
  /**
   * 浏览器工具的实时执行时间线（如「正在打开：xxx」「正在点击：button "登录"」）。
   * 仅实时管道产生（browser-status 事件累积，相邻去重、容量 capped）；
   * 历史投影不带——回看时以最终 output 为准。
   */
  browserActivity?: string[];
}

export function toolRunStatusToCallStatus(status: string): ToolCallStatus {
  switch (status) {
    case "succeeded":
    case "success":
      return "success";
    case "recoverable_error":
    case "fatal_error":
    case "internal_error":
    case "cancelled":
    case "failed":
    case "error":
      return "error";
    default:
      return "running";
  }
}

/**
 * 按运行 ID 覆盖最新快照，并输出稳定的父子深度优先顺序。
 * sequence 只在同一父节点内排序，避免不同层级的序号互相干扰。
 */
export function mergeToolRunRecords(
  current: readonly DispatcherToolRunRecord[],
  incoming: readonly DispatcherToolRunRecord[],
): DispatcherToolRunRecord[] {
  const byId = new Map(current.map((run) => [run.id, run] as const));
  for (const run of incoming) byId.set(run.id, run);

  const runs = [...byId.values()];
  const childrenByParent = new Map<string, DispatcherToolRunRecord[]>();
  const roots: DispatcherToolRunRecord[] = [];

  for (const run of runs) {
    const parentRunId = run.parentRunId ?? null;
    if (!parentRunId || !byId.has(parentRunId)) {
      roots.push(run);
      continue;
    }
    const siblings = childrenByParent.get(parentRunId) ?? [];
    siblings.push(run);
    childrenByParent.set(parentRunId, siblings);
  }

  roots.sort(compareToolRuns);
  for (const siblings of childrenByParent.values()) siblings.sort(compareToolRuns);

  const ordered: DispatcherToolRunRecord[] = [];
  const visited = new Set<string>();
  const visit = (run: DispatcherToolRunRecord) => {
    if (visited.has(run.id)) return;
    visited.add(run.id);
    ordered.push(run);
    for (const child of childrenByParent.get(run.id) ?? []) visit(child);
  };
  for (const root of roots) visit(root);
  // 数据损坏形成环时仍保留记录；visited 同时阻止递归失控。
  for (const run of runs.sort(compareToolRuns)) visit(run);
  return ordered;
}

function compareToolRuns(a: DispatcherToolRunRecord, b: DispatcherToolRunRecord): number {
  const sequence = a.sequence - b.sequence;
  if (sequence !== 0) return sequence;
  const createdAt = a.createdAt.localeCompare(b.createdAt);
  return createdAt !== 0 ? createdAt : a.id.localeCompare(b.id);
}

/** 按 id 归并一条活动进列表；缺省字段保留既有值（两套管线共用的 upsert 口径）。 */
export function upsertToolActivity(tools: ToolActivityItem[], incoming: ToolActivityItem) {
  const index = tools.findIndex((tool) => tool.id === incoming.id);
  if (index < 0) {
    tools.push(incoming);
    return;
  }

  tools[index] = {
    ...tools[index],
    ...incoming,
    input: incoming.input ?? tools[index].input,
    output: incoming.output ?? tools[index].output,
    errorText: incoming.errorText ?? tools[index].errorText,
    durationMs: incoming.durationMs ?? tools[index].durationMs,
    detailRefs: incoming.detailRefs ?? tools[index].detailRefs,
    resultMode: incoming.resultMode ?? tools[index].resultMode,
    workspaceId: incoming.workspaceId ?? tools[index].workspaceId,
    runId: incoming.runId ?? tools[index].runId,
    toolRuns:
      incoming.toolRuns == null
        ? tools[index].toolRuns
        : mergeToolRunRecords(tools[index].toolRuns ?? [], incoming.toolRuns),
    startedAtMs: tools[index].startedAtMs ?? incoming.startedAtMs,
  };
}

export function getToolErrorText(output: string): string | undefined {
  const trimmed = output.trim();
  if (/^(错误：|错误:|error:|failed:|失败：|失败:)/i.test(trimmed)) return trimmed;

  try {
    const parsed = JSON.parse(trimmed) as unknown;
    if (parsed && typeof parsed === "object" && "error" in parsed) {
      const error = (parsed as { error?: unknown }).error;
      if (typeof error === "string" && error.trim()) return error.trim();
    }
  } catch {
    // 普通文本结果不是异常，只有明确错误前缀或 error 字段才进入失败态。
  }

  return undefined;
}

export function prettyPrintToolPayload(raw: string | undefined): string {
  if (!raw) {
    return "";
  }

  try {
    const parsed = JSON.parse(raw);
    return JSON.stringify(parsed, null, 2);
  } catch {
    return raw;
  }
}

// ── 工具活动语义聚合（UI-12） ────────────────────────────────────────────────

export type ToolVerbCategory =
  | "read"
  | "write"
  | "command"
  | "search"
  | "subagent"
  | "browser"
  | "image"
  | "workflow"
  | "program"
  | "mcp"
  | "other";

/** 名字→动词类别。全集来自后端工具策略表（`agent/rig_ext/tools/spec.rs`）。 */
const TOOL_VERB_MAP: Record<string, ToolVerbCategory> = {
  read_file: "read",
  list_dir: "read",
  write_file: "write",
  edit_file: "write",
  exec: "command",
  local_zsh: "command",
  ssh_exec: "command",
  sync_directory: "command",
  grep: "search",
  glob: "search",
  call_sub_agent: "subagent",
  submit_workflow: "workflow",
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

/**
 * 摘要**只由结构化事实生成**（工具名、状态、耗时——02-design §5.2），
 * 不解析、不猜测工具输出内容；聚合结果可由同一 items 复算（验收条款）。
 * 失败/等待/运行中计数单列，由调用方以 StatusPill 双编码露出，
 * 保证「折叠不隐藏错误」。
 */
export function summarizeToolActivity(items: readonly ToolActivityItem[]): ToolActivitySummary {
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
  "workflow",
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
  workflow: (n) => `工作流 ${n}`,
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

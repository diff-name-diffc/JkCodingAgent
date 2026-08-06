import type {
  AgentActivity,
  GraphDefinition,
  GraphNodeStatus,
  GraphPlanRecord,
  GraphPlanStatus,
} from "../../types";

/** submit_graph 工具输出文本中的 plan_id 锚点（与后端 graph_submit.rs 文案对齐）。 */
const PLAN_ID_PATTERN = /plan_id=([A-Za-z0-9_-]+)/;

export function parseGraphPlanId(text: string): string | null {
  const match = PLAN_ID_PATTERN.exec(text);
  return match?.[1] ?? null;
}

/** 解析计划的图定义；定义缺失或 JSON 损坏时返回 null（调用方降级展示）。 */
export function parseGraphDefinition(plan: GraphPlanRecord | null): GraphDefinition | null {
  if (!plan) return null;
  try {
    const parsed: unknown = JSON.parse(plan.definitionJson);
    if (
      !parsed ||
      typeof parsed !== "object" ||
      !Array.isArray((parsed as GraphDefinition).nodes)
    ) {
      return null;
    }
    return parsed as GraphDefinition;
  } catch {
    return null;
  }
}

/** 解析共享 state 快照（stateJson），损坏时回退空对象。 */
export function parseGraphState(plan: GraphPlanRecord | null): Record<string, unknown> {
  if (!plan) return {};
  try {
    const parsed: unknown = JSON.parse(plan.stateJson);
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      return parsed as Record<string, unknown>;
    }
  } catch {
    // fall through
  }
  return {};
}

export const PLAN_STATUS_META: Record<GraphPlanStatus, { label: string; className: string }> = {
  draft: { label: "待确认", className: "ai-graph-chip--draft" },
  running: { label: "运行中", className: "ai-graph-chip--running" },
  completed: { label: "已完成", className: "ai-graph-chip--completed" },
  failed: { label: "有失败", className: "ai-graph-chip--failed" },
  cancelled: { label: "已取消", className: "ai-graph-chip--cancelled" },
};

export const NODE_STATUS_META: Record<GraphNodeStatus, { label: string }> = {
  pending: { label: "等待" },
  running: { label: "运行中" },
  succeeded: { label: "成功" },
  failed: { label: "失败" },
  skipped: { label: "跳过" },
  cancelled: { label: "取消" },
};

export function normalizePlanStatus(status: string): GraphPlanStatus {
  return status in PLAN_STATUS_META ? (status as GraphPlanStatus) : "draft";
}

/**
 * Kahn 拓扑分层：第 0 层为无依赖节点，同层节点可并行。
 * 用于头部统计（并行数 = 最大层宽）；环/缺失依赖的节点归入末层兜底。
 */
export function computeGraphLayers(definition: GraphDefinition): string[][] {
  const nodeIds = new Set(definition.nodes.map((node) => node.id));
  const indegree = new Map<string, number>();
  const downstream = new Map<string, string[]>();
  for (const node of definition.nodes) {
    const deps = node.dependsOn.filter((dep) => nodeIds.has(dep));
    indegree.set(node.id, deps.length);
    for (const dep of deps) {
      const list = downstream.get(dep) ?? [];
      list.push(node.id);
      downstream.set(dep, list);
    }
  }

  const layers: string[][] = [];
  let frontier = definition.nodes
    .filter((node) => (indegree.get(node.id) ?? 0) === 0)
    .map((node) => node.id);
  const placed = new Set<string>();
  while (frontier.length > 0) {
    layers.push(frontier);
    for (const id of frontier) placed.add(id);
    const next: string[] = [];
    for (const id of frontier) {
      for (const child of downstream.get(id) ?? []) {
        const remaining = (indegree.get(child) ?? 0) - 1;
        indegree.set(child, remaining);
        if (remaining === 0) next.push(child);
      }
    }
    frontier = next;
  }
  // 环等异常场景：未入层节点并入末层，保证统计不丢节点。
  const leftovers = definition.nodes.filter((node) => !placed.has(node.id)).map((node) => node.id);
  if (leftovers.length > 0) layers.push(leftovers);
  return layers;
}

/** 连线状态：等待依赖 / 已就绪（上游全部成功、下游待跑）/ 执行中 / 成功 / 失败。 */
export type GraphEdgeState = "waiting" | "ready" | "active" | "done" | "failed";

export function computeEdgeState(
  sourceStatus: GraphNodeStatus,
  targetStatus: GraphNodeStatus,
  targetReady: boolean,
): GraphEdgeState {
  if (targetStatus === "failed") return "failed";
  if (sourceStatus === "succeeded" && targetStatus === "succeeded") return "done";
  if (sourceStatus === "succeeded" && targetStatus === "running") return "active";
  if (targetReady) return "ready";
  return "waiting";
}

/** 各连线状态对应的颜色令牌（同时用于边 stroke 与箭头 marker 的 inline style）。 */
export const EDGE_STATE_COLOR: Record<GraphEdgeState, string> = {
  waiting: "var(--border-strong)",
  ready: "var(--info)",
  active: "var(--accent)",
  done: "var(--success)",
  failed: "var(--danger)",
};

export function normalizeNodeStatus(status: string): GraphNodeStatus {
  return status in NODE_STATUS_META ? (status as GraphNodeStatus) : "pending";
}

export function formatGraphDuration(durationMs: number | null | undefined): string {
  if (durationMs == null) return "";
  if (durationMs < 1000) return `${Math.round(durationMs)}ms`;
  const secs = durationMs / 1000;
  if (secs < 60) return `${secs.toFixed(1)}s`;
  return `${Math.floor(secs / 60)}m ${Math.round(secs % 60)}s`;
}

/** state 值的预览：对象/数组走 JSON 压缩，字符串截断。 */
export function previewStateValue(value: unknown, maxChars = 240): string {
  const raw =
    typeof value === "string" ? value : (JSON.stringify(value) ?? String(value));
  return raw.length > maxChars ? `${raw.slice(0, maxChars)}…` : raw;
}

/**
 * 工具输入/输出的美观展示：
 * - 对象/数组 → 两空格缩进的 JSON；
 * - 字符串若是 JSON 文本（工具参数常被序列化成字符串）→ 解析后还原换行/转义，
 *   对象则美化缩进，纯字符串则直接展示；
 * - 其余字符串原样返回（pre-wrap 会保留真实换行）。
 */
export function formatToolPayload(value: unknown): string {
  if (value == null) return "";
  if (typeof value === "string") {
    const trimmed = value.trim();
    const looksLikeJson =
      (trimmed.startsWith("{") && trimmed.endsWith("}")) ||
      (trimmed.startsWith("[") && trimmed.endsWith("]")) ||
      (trimmed.length >= 2 && trimmed.startsWith('"') && trimmed.endsWith('"'));
    if (looksLikeJson) {
      try {
        const parsed: unknown = JSON.parse(trimmed);
        if (typeof parsed === "string") return parsed;
        return JSON.stringify(parsed, null, 2) ?? value;
      } catch {
        // 不是合法 JSON，按原文展示
      }
    }
    return value;
  }
  try {
    return JSON.stringify(value, null, 2) ?? String(value);
  } catch {
    return String(value);
  }
}

/** 字符数压缩展示：1234 → 1.2k。 */
export function formatCharCount(count: number): string {
  if (count < 1000) return String(count);
  if (count < 10_000) return `${(count / 1000).toFixed(1)}k`;
  return `${Math.round(count / 1000)}k`;
}

// ── 节点执行详情：工具调用卡片数据 ──

export type ToolCallStatus = "running" | "succeeded" | "failed";

export interface ToolCallEntry {
  id: string;
  sequence: number;
  name: string;
  status: ToolCallStatus;
  inputFormatted: string;
  outputFormatted: string;
  inputChars: number;
  outputChars: number;
  durationMs: number | null;
}

export function normalizeToolCallStatus(status: string): ToolCallStatus {
  if (status === "finished") return "succeeded";
  if (status === "failed") return "failed";
  return "running"; // started / updated
}

function payloadOf(activity: AgentActivity): Record<string, unknown> {
  try {
    const parsed: unknown = JSON.parse(activity.payloadJson);
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      return parsed as Record<string, unknown>;
    }
  } catch {
    // payload 损坏时按空处理，展示不受影响
  }
  return {};
}

/** 输出原文：优先取 activity.content（result 文本），缺失时回退 payload.result。 */
function rawOutputOf(activity: AgentActivity, payload: Record<string, unknown>): string {
  if (activity.content) return activity.content;
  const result = payload.result;
  if (result == null) return "";
  return typeof result === "string" ? result : (JSON.stringify(result) ?? "");
}

/** 字符数统计口径：字符串直接取长度，其余值按 JSON 序列化后长度计。 */
function charCountOf(value: unknown): number {
  if (value == null) return 0;
  if (typeof value === "string") return value.length;
  return JSON.stringify(value)?.length ?? 0;
}

/** 把节点活动流过滤为工具调用卡片数据（按执行顺序）。 */
export function buildToolCallEntries(activities: AgentActivity[]): ToolCallEntry[] {
  return activities
    .filter((activity) => activity.kind === "tool_call")
    .sort((left, right) => left.sequence - right.sequence)
    .map((activity) => {
      const payload = payloadOf(activity);
      const inputSource = payload.args ?? payload.input ?? null;
      const outputRaw = rawOutputOf(activity, payload);
      return {
        id: activity.id,
        sequence: activity.sequence,
        name: activity.title || "工具调用",
        status: normalizeToolCallStatus(activity.status),
        inputFormatted: formatToolPayload(inputSource),
        outputFormatted: formatToolPayload(outputRaw),
        inputChars: charCountOf(inputSource),
        outputChars: charCountOf(outputRaw),
        durationMs: activity.finishedAt != null ? Math.max(0, activity.finishedAt - activity.startedAt) : null,
      };
    });
}

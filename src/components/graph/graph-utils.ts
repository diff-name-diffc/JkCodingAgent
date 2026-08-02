import type {
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
  confirmed: { label: "已确认", className: "ai-graph-chip--confirmed" },
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

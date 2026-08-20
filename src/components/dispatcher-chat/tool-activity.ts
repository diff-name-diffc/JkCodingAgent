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
 * 工具调用活动在聊天消息流中的展示模型。
 * 由 dispatcherChatView 的 live/finalized 管道生成，经 tool-call-card 渲染。
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
  /** 仅用于计算流式工具调用耗时，不属于 ToolCallCard 的展示契约。 */
  startedAtMs?: number;
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

// ── Graph Orchestrator（图编排 Agent） ──────────────────────────────────────
// 字段名严格对齐 src-tauri/src/agent/graph/types.rs（serde camelCase）。
// 修改任一字段必须同步 Rust struct。

export type GraphPlanStatus = "draft" | "running" | "completed" | "failed" | "cancelled";

export type GraphNodeStatus =
  "pending" | "running" | "succeeded" | "failed" | "skipped" | "cancelled";

export type GraphKnownNodePhase =
  | "starting"
  | "thinking"
  | "responding"
  | "tool_running"
  | "retrying"
  | "compacting"
  | "cached"
  | "finalizing";
/**
 * 应用侧阶段使用固定词表；PI sidecar lifecycle 事件允许透传额外阶段。
 * 交互层必须为未知值提供展示兜底，不能假设这是封闭枚举。
 */
export type GraphNodePhase = GraphKnownNodePhase | (string & {});
export type GraphBaseToolGroup = "read_only" | "coding";
/** 节点输出对下游的导出策略：summary=仅产出摘要段（默认），full=全文。 */
export type GraphExportPolicy = "summary" | "full";
export type GraphToolRef = { source: "pi_extension" | "aha"; name: string };

export interface GraphStateKey {
  key: string;
  description: string;
}

/** 修复图继承来源：新 plan 从既有 plan 的某次 run 继承共享 state。 */
export interface GraphInherits {
  planId: string;
  runId: string;
}

/** GraphDefinition 中的节点定义（GraphNode）。 */
export interface GraphNodeDef {
  id: string;
  title: string;
  role: string;
  modelRef: string;
  baseToolGroup: GraphBaseToolGroup;
  specialTools: GraphToolRef[];
  task: string;
  dependsOn: string[];
  injectStateKeys: string[];
  outputKey: string;
  /** 预期读写的文件（供并行写冲突预检）。 */
  expectedFiles?: string[];
  /** 输出对下游的导出策略（默认 summary）。 */
  exportPolicy?: GraphExportPolicy;
}

/** 项目 Agent 的核心产物：执行图 DAG 定义（definitionJson 解析后的结构）。 */
export interface GraphDefinition {
  version: 3;
  title: string;
  summary: string;
  stateKeys: GraphStateKey[];
  nodes: GraphNodeDef[];
  /** 修复图继承来源（可选）。 */
  inheritsFrom?: GraphInherits;
}

export interface GraphNodeRunRecord {
  runId: string;
  planId: string;
  nodeId: string;
  status: GraphNodeStatus;
  phase: GraphNodePhase;
  modelRef: string;
  modelLabel: string;
  modelCategory: string;
  baseToolGroup: GraphBaseToolGroup;
  specialToolsJson: string;
  inputText: string;
  outputText: string;
  errorText: string | null;
  startedAt: number | null;
  finishedAt: number | null;
  durationMs: number | null;
  /** 从受控写文件工具结构化参数中提取的节点影响文件。 */
  affectedFiles: string[];
  usageJson: string;
  toolCallCount: number;
  /** 已消耗的失败重试次数。 */
  retryCount: number;
}

export interface GraphRunSummary {
  id: string;
  planId: string;
  attemptNo: number;
  status: GraphPlanStatus;
  /** full=完整执行，resume=断点续跑。 */
  mode: "full" | "resume";
  /** 验收结论；历史空串在 Rust 读取层统一归一为 unknown。 */
  verdictStatus: "pass" | "partial" | "fail" | "unknown";
  verdictReason: string;
  startedAt: number;
  finishedAt: number | null;
}
export interface AgentActivity {
  id: string;
  runId: string;
  nodeId: string;
  sequence: number;
  kind: string;
  status: string;
  title: string;
  content: string;
  payloadJson: string;
  startedAt: number;
  finishedAt: number | null;
}
export interface GraphRunDetail {
  run: GraphRunSummary;
  nodeRuns: GraphNodeRunRecord[];
  activities: AgentActivity[];
}
export interface GraphHarnessModel {
  id: string;
  label: string;
  model: string;
  category: "text" | "vision";
  capabilities: string[];
}
export interface GraphHarnessTool {
  source: "pi_extension" | "aha";
  name: string;
  description: string;
  provider: string;
  category: string;
  readonly: boolean;
  reviewRequired: boolean;
}
export interface GraphHarnessCatalog {
  models: GraphHarnessModel[];
  tools: GraphHarnessTool[];
  diagnostics: string[];
}

export interface GraphPlanRecord {
  id: string;
  workspaceId: string;
  title: string;
  summary: string;
  /** 图定义原文（GraphDefinition 的 JSON 字符串）。 */
  definitionJson: string;
  status: GraphPlanStatus;
  /** 共享 state 最新快照（JSON 对象：key → 节点产出摘要；全文在节点运行记录中）。 */
  stateJson: string;
  /** 提交时刻的需求快照。 */
  requirement: string;
  inheritsPlanId: string | null;
  inheritsRunId: string | null;
  createdAt: number;
  updatedAt: number;
  latestRunId: string | null;
  runs: GraphRunSummary[];
  nodeRuns: GraphNodeRunRecord[];
}

/** `graph-plan-updated` 全局事件载荷。 */
export interface GraphPlanUpdatedPayload {
  planId: string;
  workspaceId: string;
}

// ── graph-run-event data 变体（#[serde(tag = "event", content = "data")]） ──

export interface GraphRunStartedData {
  title: string;
  attemptNo: number;
  nodeCount: number;
}

export interface GraphNodeStartedData {
  nodeId: string;
  title: string;
  modelRef: string;
  modelLabel: string;
  input: string;
}

export interface GraphNodePhaseChangedData {
  nodeId: string;
  phase: GraphNodePhase;
}
export interface GraphNodeActivityData {
  nodeId: string;
  activity: AgentActivity;
}

export interface GraphNodeOutputDeltaData {
  nodeId: string;
  delta: string;
}

export interface GraphNodeFinishedData {
  nodeId: string;
  output: string;
  durationMs: number;
  /** 节点影响文件（后端 git status 快照差分采集）。 */
  affectedFiles: string[];
}

export interface GraphNodeFailedData {
  nodeId: string;
  error: string;
  durationMs: number;
  /** 节点影响文件（后端 git status 快照差分采集；取消分支恒为空）。 */
  affectedFiles: string[];
}

export interface GraphNodeSkippedData {
  nodeId: string;
  reason: string;
}

/** 节点因运行取消而终止（区别于上游失败导致的 nodeSkipped）。 */
export interface GraphNodeCancelledData {
  nodeId: string;
}

export interface GraphStateUpdatedData {
  nodeId: string;
  key: string;
  value: string;
  /** 全量共享 state 对象。 */
  state: Record<string, unknown>;
}

export interface GraphRunFinishedData {
  state: Record<string, unknown>;
  failedNodes: string[];
  skippedNodes: string[];
}

export interface GraphRunFailedData {
  error: string;
}

/** 高危写检查点：就绪节点只剩可能写盘的节点（coding 工具组、可写特殊工具或
 * expectedFiles 任一），运行暂停等待恢复（后端 runner::node_may_write 判定）。 */
export interface GraphRunPausedData {
  nodeId: string;
}

/** runResumed/runCancelled 等无数据事件的空载荷（Rust 侧序列化为 `{}`）。 */
export type GraphRunEmptyData = Record<string, never>;

export type GraphRunEventKind =
  | "runStarted"
  | "nodeStarted"
  | "nodePhaseChanged"
  | "nodeOutputDelta"
  | "nodeActivity"
  | "nodeFinished"
  | "nodeFailed"
  | "nodeSkipped"
  | "nodeCancelled"
  | "stateUpdated"
  | "runPaused"
  | "runResumed"
  | "runFinished"
  | "runFailed"
  | "runCancelled";

/** `graph-run-event` 全局事件载荷（判别联合，按 event 收窄 data）。 */
export type GraphRunEventPayload = {
  planId: string;
  runId: string;
  workspaceId: string;
  sequence: number;
  timestampMs: number;
} & (
  | { event: "runStarted"; data: GraphRunStartedData }
  | { event: "nodeStarted"; data: GraphNodeStartedData }
  | { event: "nodePhaseChanged"; data: GraphNodePhaseChangedData }
  | { event: "nodeOutputDelta"; data: GraphNodeOutputDeltaData }
  | { event: "nodeActivity"; data: GraphNodeActivityData }
  | { event: "nodeFinished"; data: GraphNodeFinishedData }
  | { event: "nodeFailed"; data: GraphNodeFailedData }
  | { event: "nodeSkipped"; data: GraphNodeSkippedData }
  | { event: "nodeCancelled"; data: GraphNodeCancelledData }
  | { event: "stateUpdated"; data: GraphStateUpdatedData }
  | { event: "runPaused"; data: GraphRunPausedData }
  | { event: "runResumed"; data: GraphRunEmptyData }
  | { event: "runFinished"; data: GraphRunFinishedData }
  | { event: "runFailed"; data: GraphRunFailedData }
  | { event: "runCancelled"; data: GraphRunEmptyData }
);

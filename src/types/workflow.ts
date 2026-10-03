// ── Workflow Orchestrator（工作流编排 Agent） ──────────────────────────────────────
// 字段名严格对齐 src-tauri/src/agent/workflow/types.rs（serde camelCase）。
// 修改任一字段必须同步 Rust struct。

export type WorkflowPlanStatus = "draft" | "running" | "completed" | "failed" | "cancelled";

export type WorkflowNodeStatus =
  "pending" | "running" | "succeeded" | "failed" | "skipped" | "cancelled";

export type WorkflowKnownNodePhase =
  | "starting"
  | "thinking"
  | "responding"
  | "tool_running"
  | "cached"
  | "finalizing";
/**
 * 应用侧阶段使用固定词表；节点执行器（sidecar）的 lifecycle 事件允许透传额外阶段。
 * 交互层必须为未知值提供展示兜底，不能假设这是封闭枚举。
 */
export type WorkflowNodePhase = WorkflowKnownNodePhase | (string & {});
export type WorkflowBaseToolGroup = "read_only" | "coding";
/** 节点输出对下游的导出策略：summary=仅产出摘要段（默认），full=全文。 */
export type WorkflowExportPolicy = "summary" | "full";

export interface WorkflowStateKey {
  key: string;
  description: string;
}

/** 修复工作流继承来源：新 plan 从既有 plan 的某次 run 继承共享 state。 */
export interface WorkflowInherits {
  planId: string;
  runId: string;
}

/** WorkflowDefinition 中的节点定义（WorkflowNode）。 */
export interface WorkflowNodeDef {
  id: string;
  title: string;
  role: string;
  modelRef: string;
  baseToolGroup: WorkflowBaseToolGroup;
  task: string;
  dependsOn: string[];
  injectStateKeys: string[];
  outputKey: string;
  /** 预期读写的文件（供并行写冲突预检）。 */
  expectedFiles?: string[];
  /** 输出对下游的导出策略（默认 summary）。 */
  exportPolicy?: WorkflowExportPolicy;
  /** 以 plan 模式启动（先计划后执行，计划完成后自动批准并切回 bypassPermissions）；默认 false = bypassPermissions 全权限。 */
  usePlanMode?: boolean;
}

/** 项目 Agent 的核心产物：工作流 DAG 定义（definitionJson 解析后的结构）。 */
export interface WorkflowDefinition {
  version: 4;
  title: string;
  summary: string;
  stateKeys: WorkflowStateKey[];
  nodes: WorkflowNodeDef[];
  /** 修复工作流继承来源（可选）。 */
  inheritsFrom?: WorkflowInherits;
}

export interface WorkflowNodeRunRecord {
  runId: string;
  planId: string;
  nodeId: string;
  status: WorkflowNodeStatus;
  phase: WorkflowNodePhase;
  modelRef: string;
  modelLabel: string;
  modelCategory: string;
  baseToolGroup: WorkflowBaseToolGroup;
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

/** run 收尾组装的执行结果（workflow_runs 执行结果列）：结论节点输出快照 + 修改文件
 * 并集 + 结果类型。结论节点按确定性规则解析（唯一汇点；多汇点取最晚完成的
 * 成功汇点），结论节点未成功时 conclusion 为 null。 */
export interface WorkflowRunResult {
  conclusionNodeId: string | null;
  conclusionMd: string | null;
  /**
   * review=纯调研审查类；edit=含成功 coding 节点的执行写入类；
   * none=执行完成、无结果（无成功节点）；unknown 仅为历史/未收尾落库值
   * （Rust 读取层归一为 result=null，已组装结果不出现该值）。
   */
  resultKind: "review" | "edit" | "none" | "unknown" | (string & {});
  /** 本次 run 全部节点 affectedFiles 的并集（排序去重）。 */
  modifiedFiles: string[];
}

export interface WorkflowRunSummary {
  id: string;
  planId: string;
  attemptNo: number;
  status: WorkflowPlanStatus;
  /** full=完整执行，resume=断点续跑。 */
  mode: "full" | "resume";
  /** 验收结论；历史空串在 Rust 读取层统一归一为 unknown。 */
  verdictStatus: "pass" | "partial" | "fail" | "unknown";
  verdictReason: string;
  startedAt: number;
  finishedAt: number | null;
  /** 执行结果（正常收尾时组装落库，无成功节点为 resultKind="none"；取消/中断为 null）。 */
  result: WorkflowRunResult | null;
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
export interface WorkflowRunDetail {
  run: WorkflowRunSummary;
  nodeRuns: WorkflowNodeRunRecord[];
  activities: AgentActivity[];
}
export interface WorkflowHarnessModel {
  id: string;
  label: string;
  model: string;
  /** v4 起工作流节点执行器为 ACP，目录条目 category 恒为 "acp"；旧值仅见于历史数据。 */
  category: "text" | "vision" | "acp";
  capabilities: string[];
}
export interface WorkflowHarnessCatalog {
  models: WorkflowHarnessModel[];
  diagnostics: string[];
}

export interface WorkflowPlanRecord {
  id: string;
  workspaceId: string;
  title: string;
  summary: string;
  /** 工作流定义原文（WorkflowDefinition 的 JSON 字符串）。 */
  definitionJson: string;
  status: WorkflowPlanStatus;
  /** 共享 state 最新快照（JSON 对象：key → 节点产出摘要；全文在节点运行记录中）。 */
  stateJson: string;
  /** 提交时刻的需求快照。 */
  requirement: string;
  inheritsPlanId: string | null;
  inheritsRunId: string | null;
  createdAt: number;
  updatedAt: number;
  latestRunId: string | null;
  runs: WorkflowRunSummary[];
  nodeRuns: WorkflowNodeRunRecord[];
}

/** `workflow-plan-updated` 全局事件载荷。 */
export interface WorkflowPlanUpdatedPayload {
  planId: string;
  workspaceId: string;
}

/** `workflow_plan_list_for_session` 的轻量列表项（会话工作流列表页）。 */
export interface WorkflowPlanListItem {
  id: string;
  title: string;
  summary: string;
  status: WorkflowPlanStatus;
  nodeCount: number;
  createdAt: number;
  updatedAt: number;
  /** 最近一次运行摘要（未运行过为 null）。 */
  latestRun: {
    id: string;
    attemptNo: number;
    status: string;
    mode: string;
    verdictStatus: string;
    finishedAt: number | null;
    /** 执行结果类型（review/edit；none=执行完成无结果；未收尾与历史 run 为 unknown）。 */
    resultKind: string;
    /** 结论 md 预览（SQL 截取；无结论为空串）。 */
    conclusionPreview: string;
    /** 修改文件清单长度。 */
    modifiedFileCount: number;
  } | null;
}

// ── workflow-run-event data 变体（#[serde(tag = "event", content = "data")]） ──

export interface WorkflowRunStartedData {
  title: string;
  attemptNo: number;
  nodeCount: number;
}

export interface WorkflowNodeStartedData {
  nodeId: string;
  title: string;
  modelRef: string;
  modelLabel: string;
  input: string;
}

export interface WorkflowNodePhaseChangedData {
  nodeId: string;
  phase: WorkflowNodePhase;
}
export interface WorkflowNodeActivityData {
  nodeId: string;
  activity: AgentActivity;
}

export interface WorkflowNodeOutputDeltaData {
  nodeId: string;
  delta: string;
}

export interface WorkflowNodeFinishedData {
  nodeId: string;
  output: string;
  durationMs: number;
  /** 节点影响文件（ACP 工具调用声明的 locations 归一化采集）。 */
  affectedFiles: string[];
}

export interface WorkflowNodeFailedData {
  nodeId: string;
  error: string;
  durationMs: number;
  /** 节点影响文件（ACP 工具调用 locations 采集；失败/取消分支恒为空）。 */
  affectedFiles: string[];
}

export interface WorkflowNodeSkippedData {
  nodeId: string;
  reason: string;
}

/** 节点因运行取消而终止（区别于上游失败导致的 nodeSkipped）。 */
export interface WorkflowNodeCancelledData {
  nodeId: string;
}

export interface WorkflowStateUpdatedData {
  nodeId: string;
  key: string;
  value: string;
  /** 全量共享 state 对象。 */
  state: Record<string, unknown>;
}

export interface WorkflowRunFinishedData {
  state: Record<string, unknown>;
  failedNodes: string[];
  skippedNodes: string[];
}

export interface WorkflowRunFailedData {
  error: string;
}

/** 高危写检查点：就绪节点只剩可能写盘的节点（coding 工具组或
 * expectedFiles 任一），运行暂停等待恢复（后端 runner::node_may_write 判定）。 */
export interface WorkflowRunPausedData {
  nodeId: string;
}

/** runResumed/runCancelled 等无数据事件的空载荷（Rust 侧序列化为 `{}`）。 */
export type WorkflowRunEmptyData = Record<string, never>;

/** `workflow-run-event` 全局事件载荷（判别联合，按 event 收窄 data）。 */
export type WorkflowRunEventPayload = {
  planId: string;
  runId: string;
  workspaceId: string;
  sequence: number;
  timestampMs: number;
} & (
  | { event: "runStarted"; data: WorkflowRunStartedData }
  | { event: "nodeStarted"; data: WorkflowNodeStartedData }
  | { event: "nodePhaseChanged"; data: WorkflowNodePhaseChangedData }
  | { event: "nodeOutputDelta"; data: WorkflowNodeOutputDeltaData }
  | { event: "nodeActivity"; data: WorkflowNodeActivityData }
  | { event: "nodeFinished"; data: WorkflowNodeFinishedData }
  | { event: "nodeFailed"; data: WorkflowNodeFailedData }
  | { event: "nodeSkipped"; data: WorkflowNodeSkippedData }
  | { event: "nodeCancelled"; data: WorkflowNodeCancelledData }
  | { event: "stateUpdated"; data: WorkflowStateUpdatedData }
  | { event: "runPaused"; data: WorkflowRunPausedData }
  | { event: "runResumed"; data: WorkflowRunEmptyData }
  | { event: "runFinished"; data: WorkflowRunFinishedData }
  | { event: "runFailed"; data: WorkflowRunFailedData }
  | { event: "runCancelled"; data: WorkflowRunEmptyData }
);

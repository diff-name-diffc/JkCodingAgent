export interface Project {
  id: string;
  name: string;
  path: string;
  branch?: string;
  lastOpenedAt: number;
}

export type AgentType = "claude" | "codex";
export type PermissionMode = "ask" | "auto_edit" | "full_access";
export type TaskStatus =
  | "todo"
  | "pending"
  | "running"
  | "input_required"
  | "stopped"
  | "done"
  | "failed"
  | "cancelled";

export interface Task {
  id: string;
  projectId: string;
  name?: string;
  prompt: string;
  agent: AgentType;
  permissionMode: PermissionMode;
  status: TaskStatus;
  createdAt: number;
  attentionRequestedAt?: number;
  starred?: boolean;
  failureReason?: string;
  codexSessionId?: string;
  codexSessionPath?: string;
  claudeSessionId?: string;
  claudeSessionPath?: string;
  dispatcherSessionId?: string;
  dispatcherDispatchId?: string;
  dispatcherDescription?: string;
}

export type ProjectMcpAggregateStatus =
  | "not_configured"
  | "healthy"
  | "degraded"
  | "invalid_config";

export type ProjectMcpServerState =
  | "disabled"
  | "healthy"
  | "invalid_config"
  | "spawn_failed"
  | "connection_failed";

export type ProjectMcpToolTaskSupport = "forbidden" | "optional" | "required";

export interface ProjectMcpToolStatus {
  name: string;
  exposedName: string;
  description: string;
  taskSupport: ProjectMcpToolTaskSupport;
}

export interface ProjectMcpServerStatus {
  name: string;
  transport: string;
  enabled: boolean;
  state: ProjectMcpServerState;
  summary: string;
  error?: string;
  toolCount: number;
  tools: ProjectMcpToolStatus[];
}

export interface ProjectMcpStatus {
  projectPath: string;
  configPath: string;
  aggregate: ProjectMcpAggregateStatus;
  checkedAt: number;
  serverCount: number;
  enabledServerCount: number;
  healthyServerCount: number;
  servers: ProjectMcpServerStatus[];
  configError?: string;
}

export interface SshServerConfig {
  id: string;
  enabled: boolean;
  host: string;
  port: number;
  username: string;
  password: string;
  authMethod: "password" | "key";
  privateKeyPath: string;
  privateKeyPassphrase: string;
  description: string;
  tags: string[];
  reviewEnabled: boolean;
  defaultTimeoutSecs: number;
  maxOutputBytes: number;
}

export interface SshToolsConfig {
  servers: SshServerConfig[];
}

export interface SshAuditReview {
  allowed: boolean;
  reason: string;
}

export interface SshAuditRecord {
  createdAt: string;
  workspacePath: string;
  workspaceId: string;
  sessionTitle: string;
  serverId: string;
  sessionId: string;
  command: string;
  exitCode?: number | null;
  stdout: string;
  stderr: string;
  durationMs?: number | null;
  truncated: boolean;
  interactiveBlocked?: boolean;
  error?: string | null;
  review?: SshAuditReview | null;
}

export interface SshAuditLog {
  records: SshAuditRecord[];
}

export interface BrowserStatus {
  sessionId: string;
  state: "booting" | "starting" | "downloading" | "launching" | "ready" | "minimized" | "page_closed" | "closed" | string;
  url?: string | null;
  message?: string | null;
  minimized?: boolean;
  hasHeadedWindow?: boolean;
}

export interface BrowserFrameEvent {
  sessionId: string;
  data: string;
  width: number;
  height: number;
}

export interface BrowserLogEvent {
  sessionId: string;
  message: string;
}

export function isActiveTaskStatus(status: TaskStatus): boolean {
  return status === "pending" || status === "running" || status === "input_required";
}

// ── Notifications ────────────────────────────────────────────────────────────

export interface NotificationItem {
  id: string;
  notifType: "update" | "announcement" | "warning" | string;
  level: "info" | "warning" | "error" | string;
  title: string;
  body: string;
  url: string | null;
  createdAt: string;
  popup: boolean;
  isRead: boolean;
}

export interface NotificationResult {
  notifications: NotificationItem[];
  unreadCount: number;
  hasUnreadPopup: boolean;
}

// ── Content Segments ─────────────────────────────────────────────────────────

export type ContentSegmentType = "text" | "image" | "file";

export interface ContentSegment {
  id: string;
  type: ContentSegmentType;
}

export interface TextSegment extends ContentSegment {
  type: "text";
  text: string;
}

export interface ImageSegment extends ContentSegment {
  type: "image";
  imageId: string;
  path: string;
  alt?: string;
  width?: number;
  height?: number;
  mimeType?: string;
  source: "user_paste" | "tool_generate" | "file_attach";
  generationPrompt?: string;
}

export interface FileSegment extends ContentSegment {
  type: "file";
  fileId: string;
  path: string;
  fileName: string;
  mimeType: string;
  size: number;
}

export type AnyContentSegment = TextSegment | ImageSegment | FileSegment;

// ── Image Generation Tool ────────────────────────────────────────────────────
// TODO: image generation tool not yet implemented

export interface ImageGenerationInput {
  prompt: string;
  width?: number;
  height?: number;
  style?: string;
  negativePrompt?: string;
  model?: string;
  seed?: number;
}

export interface ImageGenerationOutput {
  imageId: string;
  path: string;
  width: number;
  height: number;
  mimeType: string;
  generationPrompt: string;
  generationParams: Record<string, unknown>;
  createdAt: string;
}

// ── Dispatcher Agent ─────────────────────────────────────────────────────────

export interface DispatcherMessage {
  id: string;
  workspaceId: string;
  role: "user" | "assistant" | "tool";
  segments: AnyContentSegment[];
  content: string; // derived from segments for backward compat
  thinkingContent?: string | null;
  thinkingElapsedMs?: number | null;
  toolCallId?: string;
  toolName?: string;
  toolResultMode?: DispatcherToolResultMode;
  toolArtifacts?: DispatcherToolArtifactRef[];
  toolCallsJson?: string;
  usageStats?: DispatcherMessageUsageStats | null;
  createdAt: string;
}

export interface DispatcherMessageUsageStats {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  elapsedMs: number;
  paused?: boolean;
}

export type DispatcherToolResultMode =
  | "raw"
  | "summary"
  | "conservative_summary"
  | "intent_compressed"
  | "structured_fallback"
  | "truncated"
  | "pending_summary";

export interface DispatcherToolArtifactRef {
  id: string;
  title: string;
  kind: string;
  preview: string;
  charCount: number;
  lineCount: number;
  createdAt: string;
}

export interface DispatcherToolArtifact {
  id: string;
  workspaceId: string;
  messageId?: string;
  toolCallId?: string;
  toolName?: string;
  title: string;
  kind: string;
  preview: string;
  content: string;
  charCount: number;
  lineCount: number;
  createdAt: string;
}

export interface DispatcherToolRunRecord {
  id: string;
  workspaceId: string;
  toolCallId: string;
  toolName: string;
  provider: string;
  category: string;
  status: string;
  argumentsJson: string;
  effectiveArgumentsJson: string;
  resultMode?: DispatcherToolResultMode | null;
  messageId?: string | null;
  errorKind?: string | null;
  errorMessage?: string | null;
  actionKind?: string | null;
  startedAt?: string | null;
  finishedAt?: string | null;
  durationMs: number;
  metadataJson: string;
  createdAt: string;
  updatedAt: string;
}

export interface DispatcherModelConfig {
  url: string;
  apiKey: string;
  model: string;
  active: boolean;
  systemPrompt?: string;
}

export type AgentContext = "project" | "chat";

export interface AgentToolInfo {
  name: string;
  description: string;
}

export interface AhaContextConfig {
  chatModelConfigs: DispatcherModelConfig[];
  summaryModelConfigs: DispatcherModelConfig[];
  allowedTools: string[];
}

export interface ChatCategoryAgentConfig {
  categoryId: string;
  categoryName: string;
  allowedTools: string[];
  systemPrompt: string;
  subAgentIds: string[];
  createdAt: string;
  updatedAt: string;
}

export interface AhaSharedModels {
  visionModelConfigs: DispatcherModelConfig[];
  imageModelConfigs: DispatcherModelConfig[];
  imageEditModelConfigs: DispatcherModelConfig[];
  asrModelConfigs: DispatcherModelConfig[];
  ttsModelConfigs: DispatcherModelConfig[];
  embeddingModelConfigs: DispatcherModelConfig[];
}

export interface SshReviewConfig {
  modelConfig: DispatcherModelConfig;
  systemPrompt: string;
}

/** 模型库分类：按模型调用方式划分，「模型服务」页按此分标签管理。 */
export type ModelCategory =
  | "text"
  | "vision"
  | "image"
  | "imageEdit"
  | "asr"
  | "tts"
  | "embedding";

/** 分类模型库条目：每个条目独立持有 url/apiKey/model，供「模型用途」页按分类引用。 */
export interface ModelLibraryEntry {
  id: string;
  category: ModelCategory;
  url: string;
  apiKey: string;
  model: string;
  /** 显示名，空则用 model。 */
  alias?: string;
  /** 停用后不出现在用途下拉的选项中。 */
  enabled: boolean;
}

export interface GraphExecutionConfig {
  /** 高危写检查点：每个 run 首个 coding 节点启动前暂停，等待用户恢复。 */
  pauseBeforeWrite: boolean;
}

export interface AhaSettingsV2 {
  shared: AhaSharedModels;
  project: AhaContextConfig;
  chat: AhaContextConfig;
  autoApproveDispatch: boolean;
  contextDebug: boolean;
  review: SshReviewConfig;
  modelLibrary: ModelLibraryEntry[];
  /** 执行图编排运行期设置。 */
  graph?: GraphExecutionConfig;
}

export type DispatcherSessionTokenUsageSource = "primary" | "summary";

export interface DispatcherSessionTokenUsage {
  workspaceId: string;
  model: string;
  sourceKind: DispatcherSessionTokenUsageSource;
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  cachedTokens: number;
  contextWindowTokens: number;
  contextWindowCapacity: number;
  updatedAt: string;
}

export interface DispatcherAgentTurn {
  reply: DispatcherMessage;
  messages: DispatcherMessage[];
}

export type PythonCodeRunStatus = "running" | "done" | "failed" | "stopped";

export interface PythonCodeRunRecord {
  runId: string;
  workspaceId: string;
  messageId: string;
  codeBlockIndex: number;
  codeHash: string;
  code: string;
  status: PythonCodeRunStatus | string;
  stdout: string;
  stderr: string;
  installedPackagesJson: string;
  toolEventsJson: string;
  explanationMarkdown: string;
  errorReason?: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface PythonRunToolEvent {
  kind: string;
  name: string;
  detail: string;
  createdAt: string;
}

export interface PythonRunEvent {
  event: "started" | "output" | "toolStarted" | "toolFinished" | "final" | "failed" | "stopped" | string;
  runId: string;
  workspaceId: string;
  messageId: string;
  codeBlockIndex: number;
  data: {
    record?: PythonCodeRunRecord;
    stdout?: string;
    stderr?: string;
    name?: string;
    result?: string;
    error?: string;
    message?: string;
  };
}

export interface PythonCodeRunTarget {
  messageId: string;
  codeBlockIndex: number;
  code: string;
  codeHash: string;
}

/** Maps to the Rust `AgentEvent` enum (tagged union via serde) */
export type DispatcherAgentEvent =
  | { event: "started"; data: { workspaceId: string } }
  | { event: "userMessage"; data: { message: DispatcherMessage } }
  | { event: "assistantStarted"; data: { messageId: string } }
  | {
      event: "modelSwitched";
      data: { fromModel: string; toModel: string; reason: string };
    }
  | { event: "assistantDelta"; data: { messageId: string; delta: string } }
  | {
      event: "assistantThinkingDelta";
      data: { messageId: string; delta: string; elapsedMs: number };
    }
  | { event: "assistantMessage"; data: { message: DispatcherMessage } }
  | {
      event: "runUsageUpdated";
      data: { workspaceId: string; stats: DispatcherMessageUsageStats };
    }
  | { event: "toolPlanned"; data: { toolCallId?: string; name: string; arguments: string } }
  | { event: "toolStarted"; data: { toolCallId?: string; name: string; arguments: string } }
  | {
      event: "toolSummaryStarted";
      data: {
        toolCallId?: string;
        name: string;
        resultMode: DispatcherToolResultMode;
      };
    }
  | {
      event: "toolSummaryDelta";
      data: {
        toolCallId?: string;
        name: string;
        delta: string;
        resultMode: DispatcherToolResultMode;
      };
    }
  | {
      event: "toolFinished";
      data: {
        toolCallId?: string;
        name: string;
        displayText: string;
        resultMode: DispatcherToolResultMode;
        detailRefs: DispatcherToolArtifactRef[];
      };
    }
  | { event: "toolRunUpdated"; data: { run: DispatcherToolRunRecord } }
  | { event: "finished"; data: { messages: DispatcherMessage[] } }
  | { event: "failed"; data: { workspaceId: string; message: string } };

export interface DispatcherSession {
  id: string;
  projectId: string;
  kind: "project" | "chat";
  title: string;
  category: string;
  createdAt: string;
  updatedAt: string;
  keywords?: string[];
}

export interface ChatSession {
  id: string;
  title: string;
  category: string;
  createdAt: string;
  updatedAt: string;
  keywords: string[];
  isRunning?: boolean;
}

export interface ProjectSession {
  id: string;
  projectId: string;
  title: string;
  createdAt: string;
  updatedAt: string;
  keywords: string[];
  isRunning?: boolean;
}

export interface SessionPage<T> {
  items: T[];
  total: number;
  hasMore: boolean;
  nextCursor?: string | null;
}

export interface ChatCategory {
  id: string;
  name: string;
  icon: string;
  color: string;
  sortOrder: number;
  sessionCount: number;
  createdAt: string;
  updatedAt: string;
}

export interface SessionKeyword {
  workspaceId: string;
  keyword: string;
  weight: number;
  createdAt: string;
}

export interface SessionSearchResult {
  sessionId: string;
  sessionTitle: string;
  sessionKind: "chat" | "project";
  category: string;
  keywords: string[];
  matchedKeywords: string[];
  relevanceScore: number;
  updatedAt: string;
}

export interface SubAgentModelConfig {
  inheritFromParent: boolean;
  apiBase?: string;
  apiKey?: string;
  modelName?: string;
}

export interface SubAgentConfig {
  agentId: string;
  agentName: string;
  description: string;
  systemPrompt: string;
  userPromptTemplate: string;
  allowedTools: string[];
  modelConfig: SubAgentModelConfig;
  maxIterations: number;
  maxOutputTokens: number;
  temperature: number;
  timeoutSecs: number;
  enabled: boolean;
  createdAt: number;
  updatedAt: number;
}

export interface SubAgentRecord {
  id: string;
  name: string;
  description: string;
  configJson: string;
  enabled: boolean;
  createdAt: number;
  updatedAt: number;
}

export interface SubAgentToolInfo {
  name: string;
  description: string;
}

export interface SubAgentUsage {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
}

export type SubAgentEventType =
  | "Started"
  | "ToolStarted"
  | "ToolFinished"
  | "Progress"
  | "llmDelta"
  | "UsageUpdated"
  | "Finished"
  | "Failed";

export interface SubAgentEvent {
  event: SubAgentEventType;
  data: {
    agentId?: string;
    agentName?: string;
    task?: string;
    toolName?: string;
    arguments?: Record<string, unknown>;
    resultPreview?: string;
    message?: string;
    delta?: string;
    result?: string;
    iterations?: number;
    elapsedMs?: number;
    tokenUsage?: SubAgentUsage;
    error?: string;
  };
}

export interface SubAgentEventPayload {
  sessionId: string;
  toolCallId: string;
  timestampMs: number;
  event: SubAgentEventType;
  data: SubAgentEvent["data"];
}

export interface SubAgentRunTrace {
  workspaceId: string;
  toolCallId: string;
  agentId: string;
  status: "completed" | "failed";
  eventsJson: string;
  createdAt: string;
  updatedAt: string;
}

// ── Graph Orchestrator（图编排 Agent） ──────────────────────────────────────
// 字段名严格对齐 src-tauri/src/agent/graph/types.rs（serde camelCase）。
// 修改任一字段必须同步 Rust struct。

export type GraphPlanStatus =
  | "draft"
  | "running"
  | "completed"
  | "failed"
  | "cancelled";

export type GraphNodeStatus =
  | "pending"
  | "running"
  | "succeeded"
  | "failed"
  | "skipped"
  | "cancelled";

export type GraphNodePhase =
  | "starting"
  | "thinking"
  | "responding"
  | "tool_running"
  | "retrying"
  | "compacting"
  | "finalizing";
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
  status: string;
  /** full=完整执行，resume=断点续跑。 */
  mode: string;
  /** 验收结论：pass/partial/fail/unknown（空串=未验收）。 */
  verdictStatus: string;
  verdictReason: string;
  startedAt: number;
  finishedAt: number | null;
}
export interface AgentActivity { id: string; runId: string; nodeId: string; sequence: number; kind: string; status: string; title: string; content: string; payloadJson: string; startedAt: number; finishedAt: number | null }
export interface GraphRunDetail { run: GraphRunSummary; nodeRuns: GraphNodeRunRecord[]; activities: AgentActivity[] }
export interface GraphHarnessModel { id: string; label: string; model: string; category: "text" | "vision"; capabilities: string[] }
export interface GraphHarnessTool { source: "pi_extension" | "aha"; name: string; description: string; provider: string; category: string; readonly: boolean; reviewRequired: boolean }
export interface GraphHarnessCatalog { models: GraphHarnessModel[]; tools: GraphHarnessTool[]; diagnostics: string[] }

export interface GraphPlanRecord {
  id: string;
  workspaceId: string;
  title: string;
  summary: string;
  /** 图定义原文（GraphDefinition 的 JSON 字符串）。 */
  definitionJson: string;
  status: GraphPlanStatus;
  /** 共享 state 最新快照（JSON 对象：key → 节点输出文本）。 */
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

export interface GraphNodePhaseChangedData { nodeId: string; phase: GraphNodePhase }
export interface GraphNodeActivityData { nodeId: string; activity: AgentActivity }

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

/** 高危写检查点：首个 coding 节点即将启动，运行暂停。 */
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
  | { event: "stateUpdated"; data: GraphStateUpdatedData }
  | { event: "runPaused"; data: GraphRunPausedData }
  | { event: "runResumed"; data: GraphRunEmptyData }
  | { event: "runFinished"; data: GraphRunFinishedData }
  | { event: "runFailed"; data: GraphRunFailedData }
  | { event: "runCancelled"; data: GraphRunEmptyData }
);

// ── RAG Knowledge Base ───────────────────────────────────────────────────────
// 字段名严格对齐 src-tauri/src/rag/config.rs 的 #[serde(rename_all = "camelCase")]。
// 修改任一字段必须同步 Rust struct 与 rag/src/rag_server/config.py。

/** Qdrant 连接配置（外部独立部署的向量库实例）。 */
export interface RagQdrantConfig {
  url: string;
  apiKey: string;
  collectionPrefix: string;
  timeout: number;
  denseVectorName: string;
  sparseVectorName: string;
}

/** Embedding 模型配置（走 OpenAI 兼容 API）。 */
export interface RagEmbeddingConfig {
  provider: string;
  baseUrl: string;
  apiKey: string;
  model: string;
  dimension: number;
}

/** 稀疏向量模型配置。 */
export interface RagSparseEmbeddingConfig {
  provider: string;
  model: string;
}

/** 父子分片配置。 */
export interface RagChunkingConfig {
  parentChunkSize: number;
  parentChunkOverlap: number;
  childChunkSize: number;
  childChunkOverlap: number;
  separators: string[];
}

/** OCR 配置。 */
export interface RagOcrConfig {
  enabled: boolean;
  useCuda: boolean;
  pdfImageWidthRatio: number;
  pdfImageHeightRatio: number;
}

/** RAG 知识库完整运行时配置（权威存储于 ~/.jkcodingagent/rag/config.json）。 */
export interface RagKbConfig {
  qdrant: RagQdrantConfig;
  embedding: RagEmbeddingConfig;
  sparseEmbedding: RagSparseEmbeddingConfig;
  chunking: RagChunkingConfig;
  ocr: RagOcrConfig;
  logLevel: string;
}

/** rag_save_kb_config 的返回值。 */
export interface RagKbSaveResult {
  saved: boolean;
  /** sidecar 运行中时为 true，表示配置已热推送到 Python 进程内存。 */
  reloaded: boolean;
}

/** rag_status 的返回值。 */
export interface RagRuntimeStatus {
  running: boolean;
  port?: number | null;
}

export interface RagIngestJobStartResult {
  jobId: string;
}

export type RagIngestFileStatus = "pending" | "running" | "done" | "failed";
export type RagIngestJobStatusType = "queued" | "running" | "done" | "partial" | "failed";

export interface RagIngestFileResult {
  path: string;
  status: RagIngestFileStatus;
  rawDocuments: number;
  parentChunks: number;
  childChunks: number;
  indexedPoints: number;
  error?: string | null;
}

export interface RagIngestJobStatus {
  jobId: string;
  projectId: string;
  status: RagIngestJobStatusType;
  totalFiles: number;
  completedFiles: number;
  failedFiles: number;
  createdAt: number;
  updatedAt: number;
  error?: string | null;
  files: RagIngestFileResult[];
}

export type RagLogStream = "stdout" | "stderr" | "system";
export type RagLogLevel = "debug" | "info" | "warn" | "error" | "system";

export interface RagLogEntry {
  seq: number;
  ts: number;
  stream: RagLogStream;
  level?: RagLogLevel;
  text: string;
}

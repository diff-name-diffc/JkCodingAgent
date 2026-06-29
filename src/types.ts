export interface Project {
  id: string;
  name: string;
  path: string;
  branch?: string;
  lastOpenedAt: number;
}

export type AgentType = "claude" | "codex";
export type ThemeMode = "system" | "dark" | "light";
export type PermissionMode = "ask" | "auto_edit" | "full_access";
export type DispatcherMode = "default" | "plan";
export type ChecklistStepStatus = "pending" | "in_progress" | "completed";
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

export interface UsageWindow {
  usedPercent: number;
  remainingPercent: number;
  resetAt?: number | null;
}

export interface ClaudeUsageData {
  fiveHour?: UsageWindow | null;
  sevenDay?: UsageWindow | null;
}

export interface CodexUsageData {
  email?: string | null;
  planType?: string | null;
  primary?: UsageWindow | null;
  secondary?: UsageWindow | null;
}

export type UsageSource<T> =
  | { status: "available"; data: T }
  | { status: "unavailable"; reason: string };

export interface UsageSnapshot {
  claude: UsageSource<ClaudeUsageData>;
  codex: UsageSource<CodexUsageData>;
  fetchedAt: number;
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

export interface ChecklistPlanState {
  explanation?: string;
  items: ChecklistPlanItem[];
  updatedAt: string;
}

export interface ChecklistPlanItem {
  id?: string;
  step: string;
  status: ChecklistStepStatus;
  agent?: AgentType;
  dispatchId?: string;
  subprocessTaskId?: string;
  detail?: string;
}

export type PlanInteraction =
  | { kind: "question"; id: string; question: string; options: PlanQuestionOption[] }
  | { kind: "ready"; planPath: string; title: string; summary: string };

export interface PlanQuestionOption {
  id: string;
  label: string;
  description: string;
}

export interface DispatcherSessionRuntimeState {
  mode: DispatcherMode;
  checklist?: ChecklistPlanState | null;
  planInteraction?: PlanInteraction | null;
  activePlanPath?: string | null;
}

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

export interface DispatcherSettings {
  apiBase: string;
  apiKey: string;
  model: string;
  summaryModel: string;
  visionModel: string;
  asrApiKey: string;
  asrWebsocketUrl: string;
  autoApproveDispatch: boolean;
  contextDebug: boolean;
  imageModelUrl: string;
  imageModelApiKey: string;
  imageModel: string;
  imageEditModel: string;
  chatModelConfig: DispatcherModelConfig;
  summaryModelConfig: DispatcherModelConfig;
  visionModelConfig: DispatcherModelConfig;
  imageModelConfig: DispatcherModelConfig;
  imageEditModelConfig: DispatcherModelConfig;
  asrModelConfig: DispatcherModelConfig;
  ttsModelConfig: DispatcherModelConfig;
  embeddingModelConfig: DispatcherModelConfig;
  chatModelConfigs: DispatcherModelConfig[];
  summaryModelConfigs: DispatcherModelConfig[];
  visionModelConfigs: DispatcherModelConfig[];
  imageModelConfigs: DispatcherModelConfig[];
  imageEditModelConfigs: DispatcherModelConfig[];
  asrModelConfigs: DispatcherModelConfig[];
  ttsModelConfigs: DispatcherModelConfig[];
  embeddingModelConfigs: DispatcherModelConfig[];
  allowedTools: string[];
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

export interface AhaSettingsV2 {
  shared: AhaSharedModels;
  project: AhaContextConfig;
  chat: AhaContextConfig;
  autoApproveDispatch: boolean;
  contextDebug: boolean;
  review: SshReviewConfig;
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

export type DispatchFeedbackState =
  | "round_completed"
  | "process_done"
  | "process_failed"
  | "process_cancelled";

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
  | { event: "checklistPlanUpdated"; data: { state: ChecklistPlanState } }
  | { event: "planQuestionRequested"; data: { interaction: PlanInteraction } }
  | { event: "planDocumentOpened"; data: { planPath: string } }
  | { event: "planReady"; data: { interaction: PlanInteraction } }
  | {
      event: "planImplemented";
      data: { planPath: string; implementedPath: string; summary: string };
    }
  | {
      event: "dispatchProposed";
      data: {
        dispatchId: string;
        agent: AgentType;
        description: string;
        taskPrompt: string;
        permissionMode: string;
      };
    }
  | { event: "dispatchContinue"; data: { dispatchId: string; agent: AgentType; text: string } }
  | { event: "dispatchExit"; data: { dispatchId: string; agent: AgentType; reason: string } }
  | { event: "finished"; data: { messages: DispatcherMessage[] } };

export interface DispatcherSession {
  id: string;
  projectId: string;
  kind: "project" | "chat";
  title: string;
  mode: DispatcherMode;
  activePlanPath?: string | null;
  category: string;
  createdAt: string;
  updatedAt: string;
}

export interface ChatSession {
  id: string;
  title: string;
  category: string;
  createdAt: string;
  updatedAt: string;
  isRunning?: boolean;
}

export interface ProjectSession {
  id: string;
  projectId: string;
  title: string;
  mode: DispatcherMode;
  activePlanPath?: string | null;
  createdAt: string;
  updatedAt: string;
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
  matchedKeywords: string[];
  relevanceScore: number;
  updatedAt: string;
}

export interface SubProcess {
  id: string;
  dispatchId: string;
  sessionId: string;
  agent: "claude" | "codex";
  description: string;
  status: "pending_approval" | "running" | "stopped" | "done" | "failed";
  startedAt: number;
  failureReason?: string;
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
  event: SubAgentEventType;
  data: SubAgentEvent["data"];
}

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

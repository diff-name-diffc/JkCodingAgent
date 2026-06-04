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

export interface BrowserStatus {
  sessionId: string;
  state: "booting" | "starting" | "downloading" | "launching" | "ready" | "closed" | string;
  url?: string | null;
  message?: string | null;
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
}

export type DispatcherToolResultMode = "raw" | "summary" | "conservative_summary";

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
}

export interface DispatcherModelConfig {
  url: string;
  apiKey: string;
  model: string;
  active: boolean;
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
}

export interface ProjectSession {
  id: string;
  projectId: string;
  title: string;
  mode: DispatcherMode;
  activePlanPath?: string | null;
  createdAt: string;
  updatedAt: string;
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
  createdAt: string;
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

// ── Knowledge Base ───────────────────────────────────────────────────────────

export interface KnowledgeCollection {
  id: string;
  name: string;
  rootPath: string;
  createdAt: number;
  updatedAt: number;
}

export interface KnowledgeModelConfig {
  url: string;
  apiKey: string;
  model: string;
}

export interface KnowledgeSettings {
  textModel: KnowledgeModelConfig;
  visionModel: KnowledgeModelConfig;
  embeddingModel: KnowledgeModelConfig;
}

export interface KnowledgeIngestJob {
  id: string;
  collectionId: string;
  sourceName: string;
  sourcePath: string;
  status: "running" | "done" | "failed" | "skipped" | "cancelled" | string;
  message: string;
  pagesWritten: string[];
  createdAt: number;
  updatedAt: number;
}

export interface KnowledgePageSummary {
  collectionId: string;
  path: string;
  relativePath: string;
  title: string;
  pageType: string;
  tags: string[];
  updated?: string | null;
}

export interface KnowledgePageContent {
  collectionId: string;
  path: string;
  relativePath: string;
  title: string;
  content: string;
}

export interface KnowledgeSearchResult {
  collectionId: string;
  collectionName: string;
  path: string;
  relativePath: string;
  title: string;
  pageType: string;
  snippet: string;
  score: number;
  vectorScore: number;
  tokenScore: number;
}

export interface KnowledgeVectorStats {
  collectionId: string;
  pageCount: number;
  chunkCount: number;
  dimension: number;
}

export interface KnowledgeGraphNode {
  id: string;
  label: string;
  pageType: string;
  path: string;
}

export interface KnowledgeGraphEdge {
  source: string;
  target: string;
  weight: number;
  reason: string;
}

export interface KnowledgeGraph {
  nodes: KnowledgeGraphNode[];
  edges: KnowledgeGraphEdge[];
}

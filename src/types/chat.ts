import type { ThemePreference } from "../lib/theme";

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
  /** 唯一寻址：chat-image://{imageId}。文件路径是后端 chat_images 索引的
   * 内部细节，不再进入消息载荷。 */
  imageId: string;
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

// ── Dispatcher Agent ─────────────────────────────────────────────────────────

export interface DispatcherMessage {
  id: string;
  workspaceId: string;
  role: "user" | "assistant" | "tool";
  segments: AnyContentSegment[];
  /** 展示文本：归一化时从 segments 派生（segments 是消息内容的唯一权威形态，
   * wire 层不再携带独立正文字段）。 */
  content: string;
  /** 工具消息实际回灌给 Agent 的内容；压缩时与面向用户的 content 不同。 */
  contextPayload?: string | null;
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

/** Rust `DispatcherMessageRecord` 的原始 serde 载荷；进入 UI store 前必须归一化。 */
export type DispatcherMessageWire = Omit<DispatcherMessage, "segments" | "content"> & {
  segmentsJson: string;
};

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
  toolRunId?: string | null;
  toolName?: string;
  title: string;
  kind: string;
  preview: string;
  content: string;
  charCount: number;
  lineCount: number;
  createdAt: string;
}

export type DispatcherToolRunOrigin = "model" | "tool_program";

export interface DispatcherToolRunRecord {
  id: string;
  workspaceId: string;
  toolCallId: string;
  parentRunId?: string | null;
  origin: DispatcherToolRunOrigin;
  stepId?: string | null;
  sequence: number;
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
  /** 模型库条目引用：非空时后端保存剥离 url/apiKey/model、读取时从库回填。 */
  libraryId?: string;
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
export type ModelCategory = "text" | "vision" | "image" | "imageEdit" | "asr" | "tts" | "embedding";

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
  contextDebug: boolean;
  review: SshReviewConfig;
  modelLibrary: ModelLibraryEntry[];
  /** 执行图编排运行期设置。 */
  graph?: GraphExecutionConfig;
  /** 外观主题偏好；权威源为后端 aha_get/save_settings_v2。 */
  theme?: ThemePreference;
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
  reply: DispatcherMessageWire;
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
  event:
    "started" | "output" | "toolStarted" | "toolFinished" | "final" | "failed" | "stopped" | string;
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
  | { event: "userMessage"; data: { message: DispatcherMessageWire } }
  | { event: "assistantStarted"; data: { messageId: string } }
  | {
      event: "modelSwitched";
      data: { fromModel: string; toModel: string; reason: string };
    }
  | { event: "assistantDelta"; data: { messageId: string; seq: number; delta: string } }
  | {
      event: "assistantThinkingDelta";
      data: { messageId: string; seq: number; delta: string; elapsedMs: number };
    }
  | {
      event: "assistantMessage";
      data: { message: DispatcherMessageWire; lastSeq: number | null };
    }
  | {
      event: "runUsageUpdated";
      data: { workspaceId: string; stats: DispatcherMessageUsageStats };
    }
  | { event: "toolPlanned"; data: { toolCallId: string; name: string; arguments: string } }
  | { event: "toolStarted"; data: { toolCallId: string; name: string; arguments: string } }
  | {
      event: "toolSummaryStarted";
      data: {
        toolCallId: string;
        name: string;
        resultMode: DispatcherToolResultMode;
      };
    }
  | {
      event: "toolSummaryDelta";
      data: {
        toolCallId: string;
        name: string;
        delta: string;
        resultMode: DispatcherToolResultMode;
      };
    }
  | {
      event: "toolFinished";
      data: {
        toolCallId: string;
        name: string;
        arguments: string;
        displayText: string;
        contextPayload: string;
        resultMode: DispatcherToolResultMode;
        detailRefs: DispatcherToolArtifactRef[];
      };
    }
  | { event: "toolRunUpdated"; data: { run: DispatcherToolRunRecord } }
  | {
      event: "finished";
      data: { workspaceId: string; messageCount: number };
    }
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
  sessionId: string;
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

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
export type TaskStatus =
  | "todo"
  | "pending"
  | "running"
  | "input_required"
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

// ── Dispatcher Agent ─────────────────────────────────────────────────────────

export interface DispatcherMessage {
  id: string;
  workspaceId: string;
  role: "user" | "assistant" | "tool";
  content: string;
  toolCallId?: string;
  toolName?: string;
  toolResultMode?: DispatcherToolResultMode;
  toolArtifacts?: DispatcherToolArtifactRef[];
  toolCallsJson?: string;
  createdAt: string;
}

export type DispatcherToolResultMode = "raw" | "summary" | "conservative_summary";

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
  autoApproveDispatch: boolean;
  contextDebug: boolean;
}

export interface DispatcherAgentTurn {
  reply: DispatcherMessage;
  messages: DispatcherMessage[];
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
  | { event: "assistantDelta"; data: { messageId: string; delta: string } }
  | { event: "assistantMessage"; data: { message: DispatcherMessage } }
  | { event: "toolStarted"; data: { toolCallId?: string; name: string; arguments: string } }
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
  | {
      event: "dispatchProposed";
      data: { dispatchId: string; agent: AgentType; description: string; permissionMode: string };
    }
  | { event: "dispatchContinue"; data: { dispatchId: string; agent: AgentType; text: string } }
  | { event: "dispatchExit"; data: { dispatchId: string; agent: AgentType; reason: string } }
  | { event: "finished"; data: { messages: DispatcherMessage[] } };

export interface DispatcherSession {
  id: string;
  projectId: string;
  title: string;
  createdAt: string;
  updatedAt: string;
}

export interface SubProcess {
  id: string;
  dispatchId: string;
  sessionId: string;
  agent: "claude" | "codex";
  description: string;
  status: "pending_approval" | "running" | "done" | "failed";
  startedAt: number;
}

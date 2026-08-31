export interface Project {
  id: string;
  name: string;
  path: string;
  branch?: string;
  lastOpenedAt: number;
}

/** `project_delete` 命令返回值：被级联删除的会话 id，供前端清理内存 store 残留状态。 */
export interface ProjectDeleteResult {
  deletedSessionIds: string[];
}

/** MCP 配置作用域：全局（所有聊天共享）或项目（全局 ∪ 项目 mcp.json）。 */
export type McpScopeKind = "global" | "project";

export type McpAggregateStatus = "not_configured" | "healthy" | "degraded" | "invalid_config";

export type McpServerState =
  "disabled" | "healthy" | "invalid_config" | "spawn_failed" | "connection_failed";

export type McpToolTaskSupport = "forbidden" | "optional" | "required";

export interface McpToolStatus {
  name: string;
  exposedName: string;
  description: string;
  taskSupport: McpToolTaskSupport;
}

export interface McpServerStatus {
  name: string;
  transport: string;
  enabled: boolean;
  state: McpServerState;
  summary: string;
  error?: string;
  toolCount: number;
  tools: McpToolStatus[];
}

export interface McpStatus {
  scope: McpScopeKind;
  /** 项目作用域为项目根路径；全局作用域无。 */
  projectPath?: string;
  /** 项目作用域为项目级 mcp.json 路径；全局配置存于应用数据库，无。 */
  configPath?: string;
  aggregate: McpAggregateStatus;
  checkedAt: number;
  serverCount: number;
  enabledServerCount: number;
  healthyServerCount: number;
  servers: McpServerStatus[];
  configError?: string;
}

/** MCP 服务器配置（全局注册表与项目级 mcp.json 共用同一形状）。 */
export interface McpServerConfig {
  enabled?: boolean;
  transport?: string;
  command?: string;
  args: string[];
  env: Record<string, string>;
  cwd?: string;
  url?: string;
  socketPath?: string;
  headers: Record<string, string>;
  startupTimeoutSeconds?: number;
}

export interface McpConfig {
  mcpServers: Record<string, McpServerConfig>;
}

export interface SshServerConfig {
  id: string;
  /** 显示名称（支持中文等任意字符），仅用于界面展示；留空时界面回退展示 id。 */
  name: string;
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
  state:
    | "booting"
    | "starting"
    | "downloading"
    | "launching"
    | "ready"
    | "minimized"
    | "page_closed"
    | "closed"
    | string;
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

// ── Notifications ────────────────────────────────────────────────────────────

export interface NotificationItem {
  id: string;
  notifType: "update" | "announcement" | "warning" | string;
  level: "info" | "warning" | "error" | string;
  title: string;
  body: string;
  url: string | null;
  createdAt: string;
  isRead: boolean;
}

export interface NotificationResult {
  notifications: NotificationItem[];
  unreadCount: number;
}

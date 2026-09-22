/** sync_directory 工具参数，字段名与 Agent JSON 接口一致。 */
export interface SyncDirectory {
  ssh_profile: string;
  source: string;
  destination: string;
  delete?: boolean;
  dry_run?: boolean;
}

export interface SshSyncProgress {
  transferredBytes: number;
  percent: number;
}

export interface SshSyncProgressEvent {
  workspaceId: string;
  toolCallId: string | null;
  sshProfile: string;
  progress: SshSyncProgress;
}

/** 非零退出、超时和取消也保留此结果；不把部分同步伪装成成功。 */
export interface SshSyncResult {
  sshProfile: string;
  source: string;
  destination: string;
  delete: boolean;
  dryRun: boolean;
  exitCode: number | null;
  cancelled: boolean;
  timedOut: boolean;
  durationMs: number;
  stdout: string;
  stderr: string;
  truncated: boolean;
  progress: SshSyncProgress | null;
  auditError: string | null;
}

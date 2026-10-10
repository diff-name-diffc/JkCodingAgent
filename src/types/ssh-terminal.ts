/** ssh_term_* 工具 JSON 载荷使用 snake_case；与 Rust term 模块保持一致。 */
export interface SshTermCursor {
  row: number;
  col: number;
}

export interface SshTermHandlePayload {
  term_id: string;
  screen: string;
  cursor: SshTermCursor;
  tmux_session: string | null;
  exited: boolean;
  /** 终端顶层进程的退出码；未知为 null，不代表逐条 shell 命令的状态。 */
  exit_code: number | null;
  note: string | null;
}

export interface SshTermReadPayload extends SshTermHandlePayload {
  new_lines: string[];
  /** 主屏定稿行或备用屏变化；后者不表示完整输出日志。 */
  new_lines_kind: "completed_lines" | "screen_changes";
  screen_ansi?: string;
  history?: {
    /** terminal 是本地有限回滚，tmux 是远端当前活动 pane 的历史。 */
    source: "terminal" | "tmux";
    lines: string[];
    available_lines: number;
    next_offset: number | null;
    truncated: boolean;
  };
  wait_status?: "matched" | "timed_out" | "exited" | "cancelled";
  alt_screen: boolean;
  idle_ms: number;
  /** 光标行提示符与静默时间的启发式判断；false 不表示无需输入。 */
  awaiting_input: boolean;
  input_hint: string | null;
  truncated: boolean;
}

export interface SshTermInfo {
  term_id: string;
  server_id: string;
  session_id: string;
  tmux_session: string | null;
  created_at: string;
  last_activity_at: string;
  exited: boolean;
}

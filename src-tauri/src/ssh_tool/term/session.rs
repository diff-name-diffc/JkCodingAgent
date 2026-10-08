//! `TermSession`：一条 russh PTY channel 的生命周期与读写。
//!
//! 读写分离用 russh 内建的 `Channel::split()`：reader task 独占 `ChannelReadHalf`
//! 消费 `ChannelMsg`，会话持有 `ChannelWriteHalf`（发送侧方法均为 `&self`）。
//! 屏幕模型与状态用 `parking_lot::Mutex` 共享，临界区内纯内存操作（无 IO，
//! 对齐「持锁期间禁止 I/O」纪律）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use russh::{ChannelMsg, ChannelWriteHalf};
use tokio::sync::Notify;

use super::screen::TermScreen;
use super::{TermCursor, TermId, TermInfo, TermReadPayload};
use crate::ssh_tool::SshConnection;

pub(super) const TERM_TYPE: &str = "xterm-256color";
/// read 挂起等待的单次上限（设计文档 §4.3：wait_ms 0..25000）。
pub(super) const WAIT_MS_CEILING: u64 = 25_000;
/// 已拿到退出码后，等待对端补发 Close 的宽限（对齐 command_exec 口径）。
const EXIT_GRACE_SECS: u64 = 3;
/// close 需等对端确认，宽限上限（对齐 `command_exec.rs` 的 5s）。
const CLOSE_GRACE_SECS: u64 = 5;

/// 会话的不可变元数据。
pub(crate) struct TermMeta {
    pub(crate) term_id: TermId,
    pub(crate) server_id: String,
    pub(crate) session_id: String,
    pub(crate) tmux_session: Option<String>,
    pub(crate) created_at: DateTime<Utc>,
    /// 创建时刻的 Instant 锚点：last_activity_at（Instant）转绝对时间的基准。
    pub(crate) anchor_instant: Instant,
}

struct TermState {
    exited: bool,
    exit_code: Option<i32>,
    /// 断连等语义提示（reader 侧记录）。
    note: Option<String>,
    closed_locally: bool,
    /// reader 收到的帧计数：read 挂起等待的唤醒条件（配合 Notify）。
    output_frames: u64,
    last_output_at: Instant,
    /// 最近一次工具调用（open/send/read）时刻——空闲回收的活动口径。
    /// 刻意不含输出帧：tmux status-line 时钟每 15-60s 刷新输出，按输出续命
    /// 则 tmux 终端永不回收（设计文档 §6）。
    last_activity_at: Instant,
}

pub(crate) struct TermSession {
    pub(crate) meta: TermMeta,
    connection: Arc<SshConnection>,
    writer: ChannelWriteHalf<russh::client::Msg>,
    screen: Arc<Mutex<TermScreen>>,
    state: Arc<Mutex<TermState>>,
    notify: Arc<Notify>,
}

impl TermSession {
    /// 由开壳完成的 channel 组装会话并启动 reader task。
    pub(super) fn spawn(
        meta: TermMeta,
        connection: Arc<SshConnection>,
        channel: russh::Channel<russh::client::Msg>,
        cols: usize,
        rows: usize,
    ) -> Arc<Self> {
        let (reader, writer) = channel.split();
        let screen = Arc::new(Mutex::new(TermScreen::new(cols, rows)));
        let state = Arc::new(Mutex::new(TermState {
            exited: false,
            exit_code: None,
            note: None,
            closed_locally: false,
            output_frames: 0,
            last_output_at: Instant::now(),
            last_activity_at: Instant::now(),
        }));
        let notify = Arc::new(Notify::new());
        let session = Arc::new(Self {
            meta,
            connection,
            writer,
            screen: screen.clone(),
            state: state.clone(),
            notify: notify.clone(),
        });
        tokio::spawn(reader_loop(
            reader,
            screen,
            state,
            notify,
            session.connection.clone(),
        ));
        session
    }

    /// 发送按键 / 文本。写入失败（通道已关）返回 Err，由调用方映射错误。
    pub(crate) async fn send(&self, text: &str) -> Result<(), String> {
        self.touch_activity();
        self.writer
            .data_bytes(text.as_bytes().to_vec())
            .await
            .map_err(|error| error.to_string())
    }

    /// 读取双轨载荷；`wait_ms > 0` 且无新数据时挂起等待（任一帧到达 / 终止 /
    /// 超时 / 取消即醒），是模型的防轮询阀。
    pub(crate) async fn read_payload(
        &self,
        wait_ms: u64,
        cancel: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> TermReadPayload {
        self.touch_activity();
        if wait_ms > 0 {
            let deadline = Instant::now() + Duration::from_millis(wait_ms.min(WAIT_MS_CEILING));
            let start_frames = self.state.lock().output_frames;
            loop {
                // 先建 waiter 再查条件：notify_waiters 只唤醒已存在的 waiter，
                // 条件已满足时无需等待。Notified 是 !Unpin，pin 后才能在
                // select! 中以可变引用轮询。
                let waiter = self.notify.notified();
                tokio::pin!(waiter);
                let satisfied = {
                    let state = self.state.lock();
                    state.output_frames != start_frames || state.exited
                };
                if satisfied {
                    break;
                }
                let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                    break;
                };
                tokio::select! {
                    _ = &mut waiter => {}
                    _ = tokio::time::sleep(remaining) => { break; }
                    outcome = crate::shared::cancel::wait_for_cancel(cancel.clone()) => {
                        // 取消中断等待：立即返回当前已有内容。
                        let _ = outcome;
                        break;
                    }
                }
            }
        }
        self.snapshot_payload()
    }

    /// 送审用终端现场（光标行 + 尾部行），send 的审查上下文。
    pub(crate) fn screen_context(&self) -> String {
        self.screen.lock().screen_context()
    }

    /// 以当前状态组装一次双轨载荷（排空增量轨）。
    pub(crate) fn snapshot_payload(&self) -> TermReadPayload {
        let (new_lines, truncated) = {
            let mut screen = self.screen.lock();
            let batch = screen.take_new_lines();
            (batch.lines, batch.truncated)
        };
        let (screen_text, cursor, alt) = {
            let screen = self.screen.lock();
            (screen.snapshot(), screen.cursor(), screen.alt_screen())
        };
        let (exited, exit_code, note, idle_ms) = {
            let state = self.state.lock();
            let idle = state.last_output_at.elapsed().as_millis() as u64;
            (state.exited, state.exit_code, state.note.clone(), idle)
        };
        let note = self.decorate_exit_note(note, exited);
        TermReadPayload {
            term_id: self.meta.term_id.clone(),
            new_lines,
            screen: screen_text,
            cursor: TermCursor {
                row: cursor.0,
                col: cursor.1,
            },
            alt_screen: alt,
            tmux_session: self.meta.tmux_session.clone(),
            exited,
            exit_code,
            idle_ms,
            truncated,
            note,
        }
    }

    /// exited 时按 tmux 与否补充恢复指引（设计文档 §5.5）。
    fn decorate_exit_note(&self, note: Option<String>, exited: bool) -> Option<String> {
        if !exited {
            return note;
        }
        match (&self.meta.tmux_session, note) {
            (Some(name), Some(text)) => {
                Some(format!("{text}；现场可能仍在 tmux 会话 {name}，ssh_term_open 传同名 tmuxSession 可恢复"))
            }
            (Some(name), None) => Some(format!(
                "会话已结束（detach 语义）；现场保留在 tmux 会话 {name}，ssh_term_open 传同名 tmuxSession 可恢复"
            )),
            (None, note) => note,
        }
    }

    /// 关闭 channel（幂等；宽限 5s）。reader task 随通道关闭自然收口。
    pub(crate) async fn close_channel(&self) {
        {
            let mut state = self.state.lock();
            if state.closed_locally {
                return;
            }
            state.closed_locally = true;
        }
        let _ =
            tokio::time::timeout(Duration::from_secs(CLOSE_GRACE_SECS), self.writer.close()).await;
    }

    /// 底层连接句柄（tmux kill-session 等模板命令的临时通道用）。
    pub(super) fn connection(&self) -> &Arc<SshConnection> {
        &self.connection
    }

    /// 尺寸同步：channel window_change 与屏幕模型 resize 两侧一致。
    pub(crate) async fn resize(&self, cols: usize, rows: usize) -> Result<(), String> {
        self.touch_activity();
        self.writer
            .window_change(cols as u32, rows as u32, 0, 0)
            .await
            .map_err(|error| error.to_string())?;
        self.screen.lock().resize(cols, rows);
        Ok(())
    }

    pub(crate) fn info(&self) -> TermInfo {
        let (exited, last_activity_at) = {
            let state = self.state.lock();
            let since_create = state
                .last_activity_at
                .duration_since(self.meta.anchor_instant);
            (
                state.exited,
                self.meta.created_at + chrono::Duration::from_std(since_create).unwrap_or_default(),
            )
        };
        TermInfo {
            term_id: self.meta.term_id.clone(),
            server_id: self.meta.server_id.clone(),
            session_id: self.meta.session_id.clone(),
            tmux_session: self.meta.tmux_session.clone(),
            created_at: self.meta.created_at,
            last_activity_at,
            exited,
        }
    }

    pub(crate) fn is_idle_for(&self, secs: u64) -> bool {
        self.state.lock().last_activity_at.elapsed().as_secs() >= secs
    }

    fn touch_activity(&self) {
        self.state.lock().last_activity_at = Instant::now();
        // 活跃终端的底层连接同步续命，防连接池 reaper 误回收（P1）。
        self.connection.touch();
    }
}

/// reader task：消费 channel 消息直至终止，喂屏幕模型并唤醒挂起的 read。
async fn reader_loop(
    mut reader: russh::ChannelReadHalf,
    screen: Arc<Mutex<TermScreen>>,
    state: Arc<Mutex<TermState>>,
    notify: Arc<Notify>,
    connection: Arc<SshConnection>,
) {
    let mut eof_seen = false;
    let mut exit_status: Option<u32> = None;
    loop {
        let message = if eof_seen {
            // Eof 后对端通常补发 ExitStatus/Close：宽限窗口内等待，耗尽即收口。
            match tokio::time::timeout(Duration::from_secs(EXIT_GRACE_SECS), reader.wait()).await {
                Ok(message) => message,
                Err(_) => break,
            }
        } else {
            reader.wait().await
        };
        match message {
            Some(ChannelMsg::Data { data }) => {
                connection.touch();
                screen.lock().push_bytes(&data);
                {
                    let mut state = state.lock();
                    state.output_frames += 1;
                    state.last_output_at = Instant::now();
                }
                notify.notify_waiters();
            }
            // PTY 下无独立 stderr（远端 stderr 复用主流），防御性同样喂屏。
            Some(ChannelMsg::ExtendedData { data, .. }) => {
                connection.touch();
                screen.lock().push_bytes(&data);
                {
                    let mut state = state.lock();
                    state.output_frames += 1;
                    state.last_output_at = Instant::now();
                }
                notify.notify_waiters();
            }
            Some(ChannelMsg::ExitStatus {
                exit_status: status,
            }) => {
                exit_status = Some(status);
            }
            Some(ChannelMsg::Eof) => {
                eof_seen = true;
                // 退出码已到手即收口：宽限只为「ExitStatus 晚于 Eof」的罕见场景。
                if exit_status.is_some() {
                    break;
                }
            }
            Some(ChannelMsg::Close) | None => break,
            Some(_) => {}
        }
    }
    let mut state = state.lock();
    if !state.exited {
        state.exited = true;
        state.exit_code = exit_status.map(|status| status as i32);
        if state.exit_code.is_none() {
            // 无退出码即终止：区分「连接断开」与「对端未上报」，均按「结果
            // 未知」披露、不伪装成正常退出（对齐 ExternalStateUnknown 纪律）。
            state.note = Some(if connection.handle.is_closed() {
                "连接已断开，未取得退出码".into()
            } else {
                "会话终止但未取得退出码".into()
            });
        }
    }
    drop(state);
    notify.notify_waiters();
}

//! `TermSessionRegistry`：终端会话登记、配额、惰性空闲回收与级联清理。
//!
//! 「tmux 优先、avt 兜底」的 open 分路在此编排：auto 探测远端 tmux，命中则以
//! attach-or-create 进入 tmux 会话（模板命令，免 LLM 审查——命令串由应用拼装、
//! 会话名过白名单，见设计文档 §7）；未命中走裸 shell / exec 主路。tmux 启动
//! 失败（配置损坏等，探测已排除「未安装」）以「立即退出 + exit_code」诚实上报，
//! 由模型改传 `tmux=off` 自愈，不做隐式回退。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use russh::ChannelMsg;

use super::session::{TermMeta, TermSession, TERM_TYPE};
use super::{validate_tmux_session_name, TermHandlePayload, TermId, TermInfo, TermReadPayload};
use crate::ssh_tool::{SshConnection, SshSessionKey, SshSessionManager};

const PER_SERVER_LIMIT: usize = 4;
const GLOBAL_LIMIT: usize = 16;
/// 空闲回收：30 分钟无工具调用（设计文档 §6，对齐连接池口径）。
const IDLE_REAP_SECS: u64 = 30 * 60;
/// open 后等待首屏（提示符 / tmux UI / 立即退出）的窗口。
const FIRST_SCREEN_WAIT_MS: u64 = 2_000;
/// 模板命令（tmux 探测 / kill-session）channel 的收口超时。
const TEMPLATE_TIMEOUT_SECS: u64 = 5;
/// send 文本上限：对齐 ssh_tool::validation::validate_command 的 8192 口径。
const TEXT_MAX_CHARS: usize = 8_192;
/// cols / rows 夹紧区间（设计文档 §4.1）。
const COLS_RANGE: (usize, usize) = (40, 200);
const ROWS_RANGE: (usize, usize) = (10, 60);

pub(crate) enum TmuxPreference {
    Auto,
    Off,
}

pub(crate) struct TermOpenParams {
    pub(crate) server_id: String,
    pub(crate) session_id: String,
    pub(crate) cols: usize,
    pub(crate) rows: usize,
    /// 审查后的交互命令（PTY + exec 路径，不叠 tmux）。
    pub(crate) command: Option<String>,
    pub(crate) tmux: TmuxPreference,
    pub(crate) tmux_session: Option<String>,
}

#[derive(Debug)]
pub(crate) enum TermError {
    NotFound(String),
    /// 配额超限：message 已附现有会话清单引导模型先 close。
    QuotaExceeded(String),
    /// 会话已终止仍尝试写入。
    Exited(String),
    Open(String),
    Write(String),
}

/// 独立会话表：连接获取经 `open` 的 `manager` 参数注入（registry 不持有
/// manager，避免 Clone 循环）；会话建立后 TermSession 自持连接强引用。
#[derive(Clone, Default)]
pub(crate) struct TermSessionRegistry {
    inner: Arc<Mutex<HashMap<TermId, Arc<TermSession>>>>,
}

impl TermSessionRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) async fn open(
        &self,
        manager: &SshSessionManager,
        params: TermOpenParams,
    ) -> Result<TermHandlePayload, TermError> {
        self.reap_idle().await;
        let cols = params.cols.clamp(COLS_RANGE.0, COLS_RANGE.1);
        let rows = params.rows.clamp(ROWS_RANGE.0, ROWS_RANGE.1);

        // tmux 会话名：自定义值过白名单（模板命令的唯一模型可控输入）；
        // 缺省自动生成（跨连接恢复现场时模型显式传回同名值）。
        let requested_tmux = match &params.tmux_session {
            Some(name) => {
                validate_tmux_session_name(name).map_err(TermError::Open)?;
                Some(name.clone())
            }
            None => None,
        };

        // 配额（锁内只读计数 + 采快照；报错文案在锁外渲染——quota 文案生成
        // 需要遍历会话表，parking_lot Mutex 不可重入，锁内再 lock 会自锁死等）。
        let quota_error = {
            let inner = self.inner.lock();
            let per_server = inner
                .values()
                .filter(|term| term.meta.server_id == params.server_id)
                .count();
            // Some(Some(id)) = 单服务器超限；Some(None) = 全局超限；None = 未超限。
            let exceeded = if per_server >= PER_SERVER_LIMIT {
                Some(Some(params.server_id.clone()))
            } else if inner.len() >= GLOBAL_LIMIT {
                Some(None)
            } else {
                None
            };
            exceeded.map(|scope| {
                let filter_id = scope.as_deref();
                let infos: Vec<_> = inner
                    .values()
                    .filter(|term| filter_id.is_none_or(|id| term.meta.server_id == id))
                    .map(|term| term.info())
                    .collect();
                TermError::QuotaExceeded(render_quota_message(&infos))
            })
        };
        if let Some(error) = quota_error {
            return Err(error);
        }

        let server = manager
            .server_config_async(params.server_id.clone())
            .await
            .map_err(TermError::Open)?;
        let connection = manager
            .connection_for(
                SshSessionKey {
                    server_id: params.server_id.clone(),
                    session_id: params.session_id.clone(),
                },
                &server,
            )
            .await
            .map_err(TermError::Open)?;

        // tmux 分路：仅 shell 路径叠加（command 路径的交互命令本身即现场）。
        let use_tmux = params.command.is_none()
            && matches!(params.tmux, TmuxPreference::Auto)
            && probe_command(&connection, "command -v tmux").await;

        let mut channel = connection
            .handle
            .channel_open_session()
            .await
            .map_err(|error| TermError::Open(format!("创建 SSH channel 失败：{error}")))?;
        channel
            .request_pty(false, TERM_TYPE, cols as u32, rows as u32, 0, 0, &[])
            .await
            .map_err(|error| TermError::Open(format!("请求 PTY 失败：{error}")))?;

        let tmux_session = if let Some(command) = &params.command {
            channel
                .exec(true, command.as_bytes())
                .await
                .map_err(|error| TermError::Open(format!("执行命令失败：{error}")))?;
            None
        } else if use_tmux {
            let name = requested_tmux.unwrap_or_else(|| auto_tmux_session_name());
            let request = format!("tmux new -A -s {name}");
            channel
                .exec(true, request.as_bytes())
                .await
                .map_err(|error| TermError::Open(format!("tmux 启动请求失败：{error}")))?;
            Some(name)
        } else {
            channel
                .request_shell(false)
                .await
                .map_err(|error| TermError::Open(format!("打开 shell 失败：{error}")))?;
            None
        };

        let term_id = new_term_id();
        let session = TermSession::spawn(
            TermMeta {
                term_id: term_id.clone(),
                server_id: params.server_id,
                session_id: params.session_id,
                tmux_session,
                created_at: chrono::Utc::now(),
                anchor_instant: Instant::now(),
            },
            connection,
            channel,
            cols,
            rows,
        );
        // 等首屏（提示符 / tmux UI / 立即退出），PTY 被拒等异常也在此暴露。
        let payload = session.read_payload(FIRST_SCREEN_WAIT_MS, None).await;
        self.inner.lock().insert(term_id.clone(), session.clone());

        // tmux 模式立即退出（配置损坏 / 权限等罕见场景）：诚实上报 + 自愈指引。
        let note = if payload.tmux_session.is_some() && payload.exited {
            Some(format!(
                "tmux 会话立即退出（exit_code={}），可改传 tmux=off 重开裸 shell",
                payload
                    .exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "未知".into())
            ))
        } else {
            None
        };

        Ok(TermHandlePayload {
            term_id,
            screen: payload.screen,
            cursor: payload.cursor,
            tmux_session: payload.tmux_session,
            exited: payload.exited,
            exit_code: payload.exit_code,
            note,
        })
    }

    pub(crate) async fn send(&self, term_id: &str, text: &str) -> Result<(), TermError> {
        let session = self.get(term_id)?;
        // 对齐 validate_command 的 8192 字符口径（设计文档 §4.2）。
        if text.len() > TEXT_MAX_CHARS {
            return Err(TermError::Write(format!(
                "错误：text 长度不能超过 {TEXT_MAX_CHARS} 字符"
            )));
        }
        {
            let state = session.snapshot_payload();
            if state.exited {
                return Err(TermError::Exited(format!(
                    "终端已退出（exit_code={}），无法继续发送",
                    state
                        .exit_code
                        .map(|code| code.to_string())
                        .unwrap_or_else(|| "未知".into())
                )));
            }
        }
        session.send(text).await.map_err(TermError::Write)
    }

    pub(crate) async fn read(
        &self,
        term_id: &str,
        wait_ms: u64,
        cancel: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> Result<TermReadPayload, TermError> {
        let session = self.get(term_id)?;
        Ok(session.read_payload(wait_ms, cancel).await)
    }

    pub(crate) async fn close(
        &self,
        term_id: &str,
        kill_tmux: bool,
    ) -> Result<TermHandlePayload, TermError> {
        self.reap_idle().await;
        let session = self.get(term_id)?;
        session.close_channel().await;
        // 给 reader task 一点收口时间拿终态。
        let payload = session.read_payload(500, None).await;
        self.inner.lock().remove(term_id);
        if kill_tmux {
            if let Some(name) = &session.meta.tmux_session {
                run_template_command(
                    session.connection(),
                    &format!("tmux kill-session -t {name}"),
                )
                .await;
            }
        }
        Ok(TermHandlePayload {
            term_id: term_id.to_string(),
            screen: payload.screen,
            cursor: payload.cursor,
            tmux_session: session.meta.tmux_session.clone(),
            exited: payload.exited,
            exit_code: payload.exit_code,
            note: payload.note,
        })
    }

    pub(crate) fn list(&self, server_id: Option<&str>) -> Vec<TermInfo> {
        let inner = self.inner.lock();
        inner
            .values()
            .filter(|term| server_id.is_none_or(|id| term.meta.server_id == id))
            .map(|term| term.info())
            .collect()
    }

    /// term_id → server_id 映射（claims.rs 的 Ssh 资源解析用）。
    pub(crate) fn server_of(&self, term_id: &str) -> Option<String> {
        self.inner
            .lock()
            .get(term_id)
            .map(|term| term.meta.server_id.clone())
    }

    /// 级联关闭某聊天会话的全部终端（session_delete / 清空消息 / 项目删除）。
    /// tmux 会话一并 kill（远端资源不残留，对齐「不能只清本地」纪律）。
    pub(crate) async fn close_session_terms(&self, session_id: &str) {
        let targets: Vec<Arc<TermSession>> = {
            let inner = self.inner.lock();
            inner
                .values()
                .filter(|term| term.meta.session_id == session_id)
                .cloned()
                .collect()
        };
        for term in targets {
            let _ = self.close(&term.meta.term_id, true).await;
        }
    }

    fn get(&self, term_id: &str) -> Result<Arc<TermSession>, TermError> {
        self.inner.lock().get(term_id).cloned().ok_or_else(|| {
            TermError::NotFound(format!(
                "终端会话 {term_id} 不存在（可能已关闭或被空闲回收），可先 ssh_term_list 查看"
            ))
        })
    }

    /// 惰性空闲回收（对齐连接池 reaper 模式：入口顺带清理，不引后台任务）。
    /// 锁内只筛选，关闭动作在锁外执行。
    async fn reap_idle(&self) {
        let expired: Vec<Arc<TermSession>> = {
            let mut inner = self.inner.lock();
            let expired_ids: Vec<TermId> = inner
                .iter()
                .filter(|(_, term)| term.is_idle_for(IDLE_REAP_SECS))
                .map(|(id, _)| id.clone())
                .collect();
            expired_ids
                .iter()
                .filter_map(|id| inner.remove(id))
                .collect()
        };
        for term in expired {
            // tmux 会话保留现场（detach 语义）；裸终端随连接关闭终止远端进程。
            term.close_channel().await;
        }
    }

    fn quota_message(&self, server_id: Option<&str>) -> String {
        render_quota_message(&self.list(server_id))
    }
}

/// 配额报错文案：附现有会话清单引导模型先 close。
fn render_quota_message(infos: &[super::TermInfo]) -> String {
    let listing = infos
        .iter()
        .map(|info| {
            format!(
                "{}(server={}, tmux={:?}, exited={})",
                info.term_id, info.server_id, info.tmux_session, info.exited
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "错误：终端会话配额超限（每服务器 ≤{PER_SERVER_LIMIT}、全局 ≤{GLOBAL_LIMIT}）。现有会话：{listing}。请先 ssh_term_close 释放，或 ssh_term_open 传同名 tmuxSession 复用远端现场。"
    )
}

fn new_term_id() -> TermId {
    format!("term_{}", uuid::Uuid::new_v4().simple())
}

fn auto_tmux_session_name() -> String {
    format!(
        "jkagent-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..6]
    )
}

/// 在连接上跑一条应用内模板命令（tmux 探测 / kill-session），等退出码收口。
/// 命令串由应用拼装（模型可控输入仅白名单会话名），不属于 LLM 审查面（§7）。
async fn run_template_command(connection: &SshConnection, command: &str) -> Option<u32> {
    let mut channel = connection.handle.channel_open_session().await.ok()?;
    channel.exec(true, command.as_bytes()).await.ok()?;
    let deadline = Duration::from_secs(TEMPLATE_TIMEOUT_SECS);
    let mut exit_status = None;
    loop {
        match tokio::time::timeout(deadline, channel.wait()).await {
            Ok(Some(ChannelMsg::ExitStatus {
                exit_status: status,
            })) => {
                exit_status = Some(status);
            }
            Ok(Some(ChannelMsg::Eof)) | Ok(Some(ChannelMsg::Close)) | Ok(None) => {
                return exit_status;
            }
            Ok(Some(_)) => {}
            Err(_) => return exit_status,
        }
    }
}

async fn probe_command(connection: &SshConnection, command: &str) -> bool {
    run_template_command(connection, command).await == Some(0)
}

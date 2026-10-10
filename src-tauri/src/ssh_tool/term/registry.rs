//! `TermSessionRegistry`：终端会话登记、配额、惰性空闲回收与级联清理。
//!
//! SSH 启动握手与 tmux 探测/恢复由 startup 模块负责；这里只登记已成功启动
//! 的终端，并维护关闭、配额与清理语义。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;

use super::session::{TermMeta, TermSession};
use super::startup::{run_template_command, start_terminal};
use super::{validate_tmux_session_name, TermHandlePayload, TermId, TermInfo, TermReadPayload};
use crate::ssh_tool::{SshSessionKey, SshSessionManager};

const PER_SERVER_LIMIT: usize = 4;
const GLOBAL_LIMIT: usize = 16;
/// 空闲回收：30 分钟无工具调用（设计文档 §6，对齐连接池口径）。
const IDLE_REAP_SECS: u64 = 30 * 60;
/// open 后等待首屏（提示符 / tmux UI / 立即退出）的窗口。
const FIRST_SCREEN_WAIT_MS: u64 = 2_000;
/// cols / rows 夹紧区间（设计文档 §4.1）。
const COLS_RANGE: (usize, usize) = (40, 200);
const ROWS_RANGE: (usize, usize) = (10, 60);

pub(crate) enum TmuxPreference {
    Auto,
    Required,
    Off,
}

pub(crate) struct TermOpenParams {
    pub(crate) server_id: String,
    pub(crate) session_id: String,
    pub(crate) cols: usize,
    pub(crate) rows: usize,
    /// 审查后的交互命令；tmux 模式仅在新建远端会话时执行。
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
    /// 启动命令可能已在远端执行；必须先核实现场，禁止自动重跑。
    ExternalStateUnknown(String),
    Open(String),
    Read(String),
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
        if let Some(name) = &params.tmux_session {
            validate_tmux_session_name(name).map_err(TermError::Open)?;
            if matches!(params.tmux, TmuxPreference::Off) {
                return Err(TermError::Open(
                    "tmux=off 不能与 tmuxSession 同时使用；恢复现场请使用 tmux=auto".into(),
                ));
            }
        }

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

        let started = start_terminal(&connection, &params, cols, rows).await?;

        let term_id = new_term_id();
        let session = TermSession::spawn(
            TermMeta {
                term_id: term_id.clone(),
                server_id: params.server_id,
                session_id: params.session_id,
                tmux_session: started.tmux_session,
                created_at: chrono::Utc::now(),
                anchor_instant: Instant::now(),
            },
            connection,
            started.channel,
            cols,
            rows,
            &started.initial_output,
        );
        // 等首屏（提示符 / tmux UI / 立即退出），PTY 被拒等异常也在此暴露。
        let payload = session.read_payload(FIRST_SCREEN_WAIT_MS, None).await;
        if payload.tmux_session.is_some() && payload.exited {
            session.close_channel().await;
            let message = format!(
                "tmux attach 立即退出（exit_code={:?}）：{}；{}",
                payload.exit_code,
                payload.screen,
                payload.note.as_deref().unwrap_or_default(),
            );
            let name = payload.tmux_session.as_deref().unwrap_or_default();
            return Err(if started.command_started {
                super::startup::command_state_unknown(name, &message)
            } else {
                TermError::Open(format!("{message}；远端会话 {name} 可能仍在，请先查询状态"))
            });
        }
        self.inner.lock().insert(term_id.clone(), session);

        Ok(TermHandlePayload {
            term_id,
            screen: payload.screen,
            cursor: payload.cursor,
            tmux_session: payload.tmux_session,
            exited: payload.exited,
            exit_code: payload.exit_code,
            note: match (started.note, payload.note) {
                (Some(startup), Some(outcome)) => Some(format!("{startup}；{outcome}")),
                (startup, outcome) => startup.or(outcome),
            },
        })
    }

    pub(crate) async fn send(&self, term_id: &str, text: &str) -> Result<(), TermError> {
        self.send_checked(term_id, text, false).await
    }

    pub(crate) fn validate_paste_state(&self, term_id: &str) -> Result<(), TermError> {
        self.get(term_id)?
            .validate_paste_state()
            .map_err(TermError::Write)
    }

    pub(crate) async fn send_checked(
        &self,
        term_id: &str,
        text: &str,
        require_paste: bool,
    ) -> Result<(), TermError> {
        let session = self.get(term_id)?;
        super::validate_send_text(text).map_err(TermError::Write)?;
        if let Some(exit_code) = session.exit_status() {
            return Err(TermError::Exited(format!(
                "终端已退出（exit_code={}），无法继续发送",
                exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "未知".into())
            )));
        }
        session
            .send(text, require_paste)
            .await
            .map_err(TermError::Write)
    }

    /// 尺寸同步：channel window_change 与屏幕模型 resize 两侧一致（夹紧在此统一）。
    pub(crate) async fn resize(
        &self,
        term_id: &str,
        cols: usize,
        rows: usize,
    ) -> Result<(), TermError> {
        let session = self.get(term_id)?;
        let cols = cols.clamp(COLS_RANGE.0, COLS_RANGE.1);
        let rows = rows.clamp(ROWS_RANGE.0, ROWS_RANGE.1);
        if session.exit_status().is_some() {
            return Err(TermError::Exited(
                "终端已退出，无需调整尺寸（可 ssh_term_open 重开）".into(),
            ));
        }
        session.resize(cols, rows).await.map_err(TermError::Write)
    }

    pub(crate) async fn read(
        &self,
        term_id: &str,
        options: &super::TermReadOptions,
        cancel: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> Result<TermReadPayload, TermError> {
        let session = self.get(term_id)?;
        session
            .read_with_options(options, cancel)
            .await
            .map_err(TermError::Read)
    }

    pub(crate) async fn close(
        &self,
        term_id: &str,
        kill_tmux: bool,
    ) -> Result<TermHandlePayload, TermError> {
        self.reap_idle().await;
        let session = self.get(term_id)?;
        // 先确认远端 kill 成功再移除本地句柄；失败时保留登记以便重试。
        if kill_tmux {
            if let Some(name) = &session.meta.tmux_session {
                let result = run_template_command(
                    session.connection(),
                    &format!("tmux kill-session -t ={name}"),
                )
                .await
                .map_err(TermError::Write)?;
                result
                    .ensure_success("终止 tmux 会话")
                    .map_err(TermError::Write)?;
            }
        }
        session.close_channel().await;
        // 给 reader task 一点收口时间拿终态。
        let payload = session.read_payload(500, None).await;
        self.inner.lock().remove(term_id);
        let note = match (&session.meta.tmux_session, kill_tmux) {
            (Some(name), true) => Some(format!("tmux 会话 {name} 已终止，远端现场已回收")),
            (Some(name), false) => Some(format!(
                "终端已断开；tmux 会话 {name} 中仍在运行的程序可用同名 tmuxSession 重新连接"
            )),
            (None, _) => Some("裸终端已关闭，不支持断连保活或同名恢复".into()),
        };
        Ok(TermHandlePayload {
            term_id: term_id.to_string(),
            screen: payload.screen,
            cursor: payload.cursor,
            tmux_session: session.meta.tmux_session.clone(),
            exited: payload.exited,
            exit_code: payload.exit_code,
            note,
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

    /// 送审用终端现场（send 审查上下文）；会话不存在时 None。
    pub(crate) fn screen_context_of(&self, term_id: &str) -> Option<String> {
        self.get(term_id)
            .ok()
            .map(|session| session.screen_context())
    }

    /// term_id → server_id 映射（claims.rs 的 Ssh 资源解析用）。
    pub(crate) fn server_of(&self, term_id: &str) -> Option<String> {
        self.inner
            .lock()
            .get(term_id)
            .map(|term| term.meta.server_id.clone())
    }

    /// 级联关闭某聊天会话的全部终端（session_delete / 清空消息 / 项目删除）。
    /// tmux 会话一并 kill（远端资源不残留，对齐「不能只清本地」纪律）；
    /// 再经审计反查回收「已失联」的 tmux 孤儿（注册表已移除但远端还活着）——
    /// 审计仅保留最近 100 条（全局修剪窗口），被修剪的旧记录查不到，孤儿
    /// 回收按设计 best-effort 接受该窗口；连接不可达容忍失败。
    pub(crate) async fn close_session_terms(&self, manager: &SshSessionManager, session_id: &str) {
        let mut handled: std::collections::HashSet<String> = Default::default();
        let targets: Vec<Arc<TermSession>> = {
            let inner = self.inner.lock();
            inner
                .values()
                .filter(|term| term.meta.session_id == session_id)
                .cloned()
                .collect()
        };
        for term in targets {
            match self.close(&term.meta.term_id, true).await {
                Ok(_) => {
                    if let Some(name) = &term.meta.tmux_session {
                        handled.insert(name.clone());
                    }
                }
                Err(error) => {
                    eprintln!("级联终止 SSH 终端 {} 失败：{error:?}", term.meta.term_id);
                    // 删除聊天仍须释放本地句柄；远端失败留给下方审计反查重试。
                    term.close_channel().await;
                    self.inner.lock().remove(&term.meta.term_id);
                }
            }
        }
        // 孤儿回收：审计反查该会话历史 open 过的 tmux 会话名，跳过刚处理的。
        let Ok(log) = manager.load_audit_async().await else {
            return;
        };
        for record in &log.records {
            if record.session_id != session_id {
                continue;
            }
            let Some(name) = extract_tmux_session_name(&record.command) else {
                continue;
            };
            if !handled.insert(name.clone()) {
                continue;
            }
            kill_tmux_orphan(manager, &record.server_id, &name, session_id).await;
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

/// 从审计 command 串提取 tmux 会话名：匹配 `ssh_term_open tmux new -A -s <name>`。
fn extract_tmux_session_name(command: &str) -> Option<String> {
    const MARKER: &str = "tmux new -A -s ";
    let start = command.find(MARKER)? + MARKER.len();
    let rest = &command[start..];
    let end = rest.find(' ').unwrap_or(rest.len());
    let name = &rest[..end];
    validate_tmux_session_name(name)
        .ok()
        .map(|()| name.to_string())
}

/// 经服务器连接 kill 一个失联的 tmux 孤儿会话（模板命令免审，best-effort）。
async fn kill_tmux_orphan(
    manager: &SshSessionManager,
    server_id: &str,
    name: &str,
    session_id: &str,
) {
    let Ok(server) = manager.server_config_async(server_id.to_string()).await else {
        return;
    };
    let Ok(connection) = manager
        .connection_for(
            SshSessionKey {
                server_id: server_id.to_string(),
                session_id: session_id.to_string(),
            },
            &server,
        )
        .await
    else {
        return;
    };
    if let Err(error) = async {
        run_template_command(&connection, &format!("tmux kill-session -t ={name}"))
            .await?
            .ensure_success("回收孤儿 tmux 会话")
    }
    .await
    {
        eprintln!("回收 tmux 会话 {name} 失败：{error}");
    }
}

fn new_term_id() -> TermId {
    format!("term_{}", uuid::Uuid::new_v4().simple())
}

#[cfg(test)]
mod tests {
    use super::extract_tmux_session_name;

    #[test]
    fn extracts_name_from_open_audit_command() {
        assert_eq!(
            extract_tmux_session_name("ssh_term_open tmux new -A -s jkagent-install (80x24)"),
            Some("jkagent-install".to_string())
        );
        assert_eq!(
            extract_tmux_session_name("ssh_term_open tmux new -A -s jkagent-a_b-1 (120x40)"),
            Some("jkagent-a_b-1".to_string())
        );
        // 无尾部尺寸标注也能提取（取到串尾）。
        assert_eq!(
            extract_tmux_session_name("tmux new -A -s jkagent-x"),
            Some("jkagent-x".to_string())
        );
        assert_eq!(
            extract_tmux_session_name("ssh_term_open shell (80x24)"),
            None
        );
        assert_eq!(extract_tmux_session_name("ssh_term_close detach"), None);
    }
}

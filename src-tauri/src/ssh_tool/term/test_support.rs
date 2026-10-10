//! 交互终端回环 SSH 服务器与测试 fixture。

use std::sync::Arc;
use std::time::Duration;

use russh::server::{Auth, Msg, Server as _};
use russh::ChannelId;
use serde_json::json;
use tokio::net::TcpListener;

use super::super::registry::{TermOpenParams, TermSessionRegistry, TmuxPreference};
use crate::ssh_tool::SshDb;

// ---------------------------------------------------------------------------
// 回环服务器
// ---------------------------------------------------------------------------

/// 测试服务器侧事件（exec 命令 / 窗口变化），供断言远端实际收到了什么。
type ServerEvents = std::sync::Arc<parking_lot::Mutex<Vec<String>>>;

#[derive(Clone, Default)]
pub(super) struct ServerBehavior {
    pub(super) tmux_available: bool,
    pub(super) eof_before_status: bool,
    pub(super) omit_probe_status: bool,
    pub(super) probe_exit: Option<u32>,
    pub(super) reject_pty: bool,
    pub(super) reject_shell: bool,
    pub(super) create_exit: u32,
    pub(super) create_omit_status: bool,
    pub(super) attach_exit: Option<u32>,
    pub(super) kill_exit: u32,
    pub(super) query_cursor_position: bool,
    pub(super) exit_on_eof: Option<u32>,
    pub(super) command_output: Option<Vec<u8>>,
    pub(super) history_exit: u32,
    pub(super) history_omit_status: bool,
    pub(super) exec_reply_delay: Duration,
    pub(super) omit_terminal_reply: bool,
}

type TmuxSessions = Arc<parking_lot::Mutex<std::collections::HashSet<String>>>;

#[derive(Clone)]
struct LoopServer {
    behavior: ServerBehavior,
    events: ServerEvents,
    tmux_sessions: TmuxSessions,
}

impl russh::server::Server for LoopServer {
    type Handler = LoopHandler;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self::Handler {
        LoopHandler {
            behavior: self.behavior.clone(),
            events: self.events.clone(),
            tmux_sessions: self.tmux_sessions.clone(),
        }
    }
}

struct LoopHandler {
    behavior: ServerBehavior,
    events: ServerEvents,
    tmux_sessions: TmuxSessions,
}

impl russh::server::Handler for LoopHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, _: &str, _: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        _channel: russh::Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        self.events.lock().push("shell".into());
        if self.behavior.reject_shell {
            session.channel_failure(channel)?;
        } else {
            session.channel_success(channel)?;
            session.data(channel, b"$ ".to_vec())?;
            if self.behavior.query_cursor_position {
                session.data(channel, b"\x1b[4;7H\x1b[6n".to_vec())?;
            }
        }
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _: &str,
        _: u32,
        _: u32,
        _: u32,
        _: u32,
        _: &[(russh::Pty, u32)],
        session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        if self.behavior.reject_pty {
            session.channel_failure(channel)?;
        } else {
            session.channel_success(channel)?;
        }
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).to_string();
        self.events.lock().push(format!("exec: {command}"));
        tokio::time::sleep(self.behavior.exec_reply_delay).await;
        if self.behavior.omit_terminal_reply
            && (command.starts_with("tmux attach-session -t =")
                || (!command.starts_with("tmux ") && command != "command -v tmux"))
        {
            // 服务端已收到命令，但启动确认丢失；保持 channel 打开以触发客户端超时。
            self.events
                .lock()
                .push(format!("withheld-reply: {channel}"));
            return Ok(());
        }
        session.channel_success(channel)?;
        if command == "command -v tmux" {
            if self.behavior.tmux_available {
                session.data(channel, b"/usr/bin/tmux\r\n".to_vec())?;
            }
            let status = self
                .behavior
                .probe_exit
                .unwrap_or(u32::from(!self.behavior.tmux_available));
            self.finish_command(channel, status, self.behavior.omit_probe_status, session)?;
        } else if let Some(name) = command.strip_prefix("tmux has-session -t =") {
            let status = u32::from(!self.tmux_sessions.lock().contains(name));
            self.finish_command(channel, status, false, session)?;
        } else if let Some(rest) = command.strip_prefix("tmux new-session -d -s ") {
            let name = rest.split_whitespace().next().expect("name");
            if self.behavior.create_exit == 0 {
                self.tmux_sessions.lock().insert(name.to_string());
            } else {
                session.extended_data(channel, 1, b"tmux configuration error".to_vec())?;
            }
            self.finish_command(
                channel,
                self.behavior.create_exit,
                self.behavior.create_omit_status,
                session,
            )?;
        } else if let Some(name) = command.strip_prefix("tmux kill-session -t =") {
            if self.behavior.kill_exit == 0 {
                self.tmux_sessions.lock().remove(name);
            } else {
                session.extended_data(channel, 1, b"kill denied".to_vec())?;
            }
            self.finish_command(channel, self.behavior.kill_exit, false, session)?;
        } else if let Some(name) = command.strip_prefix("tmux attach-session -t =") {
            assert!(self.tmux_sessions.lock().contains(name));
            if let Some(code) = self.behavior.attach_exit {
                // 已执行的新建命令很快结束，attach 收到 ACK 后发现会话不存在。
                self.tmux_sessions.lock().remove(name);
                session.extended_data(channel, 1, b"can't find session\r\n".to_vec())?;
                self.finish_command(channel, code, false, session)?;
            } else {
                // 模拟 tmux client：attach 成功，保持通道（不退出）。
                session.data(channel, b"[tmux attached]\r\n".to_vec())?;
            }
        } else {
            session.data(
                channel,
                self.behavior
                    .command_output
                    .clone()
                    .unwrap_or_else(|| format!("run: {command}\r\n").into_bytes()),
            )?;
            if command.starts_with("tmux display-message ") {
                if self.behavior.history_exit != 0 {
                    session.extended_data(channel, 1, b"can't find pane".to_vec())?;
                }
                self.finish_command(
                    channel,
                    self.behavior.history_exit,
                    self.behavior.history_omit_status,
                    session,
                )?;
            } else {
                session.exit_status_request(channel, 0)?;
                session.eof(channel)?;
                session.close(channel)?;
            }
        }
        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        if self.behavior.omit_terminal_reply {
            self.events.lock().push(format!("close: {channel}"));
        }
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        _channel: ChannelId,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        self.events
            .lock()
            .push(format!("window-change: {col_width}x{row_height}"));
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        if self.behavior.query_cursor_position && data.starts_with(b"\x1b[") && data.ends_with(b"R")
        {
            self.events
                .lock()
                .push(format!("cpr: {}", String::from_utf8_lossy(data)));
            // 查询应答属于终端协议；不再次回显控制序列，避免伪造查询回环。
            session.data(channel, b"CPR_OK\r\n".to_vec())?;
            return Ok(());
        }
        if data == b"\x04" {
            if let Some(status) = self.behavior.exit_on_eof {
                return self.finish_command(channel, status, false, session);
            }
        }
        // 行回显 + 回车后的新提示符（模拟登录 shell 的最小行为）。
        session.data(channel, data.to_vec())?;
        if data.contains(&b'\r') {
            session.data(channel, b"\r\n$ ".to_vec())?;
        }
        Ok(())
    }
}

impl LoopHandler {
    fn finish_command(
        &self,
        channel: ChannelId,
        status: u32,
        omit_status: bool,
        session: &mut russh::server::Session,
    ) -> Result<(), russh::Error> {
        if self.behavior.eof_before_status {
            session.eof(channel)?;
        }
        if !omit_status {
            session.exit_status_request(channel, status)?;
        }
        if !self.behavior.eof_before_status {
            session.eof(channel)?;
        }
        session.close(channel)
    }
}

/// 固定测试密钥（仅测试用途，无敏感性）：随机密钥需要 rand 版本与 ssh-key 的
/// rand_core trait 对齐，固定 PEM 避开依赖纠缠。
const TEST_HOST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACDu+QkbiQ5z3ZCGBDxaVo7ZrpBlccoKej8vTkTuo/3hmQAAAKAcisBNHIrA\nTQAAAAtzc2gtZWQyNTUxOQAAACDu+QkbiQ5z3ZCGBDxaVo7ZrpBlccoKej8vTkTuo/3hmQ\nAAAED9lkd8MRooMXd6QPfjYxgEdtIJodhCcWZvfIlRACLsku75CRuJDnPdkIYEPFpWjtmu\nkGVxygp6Py9ORO6j/eGZAAAAGmprQGprcy1NYWNCb29rLVByby01LmxvY2FsAQID\n-----END OPENSSH PRIVATE KEY-----";

async fn spawn_loop_server(behavior: ServerBehavior) -> (u16, ServerEvents, TmuxSessions) {
    let config = Arc::new(russh::server::Config {
        keys: vec![russh::keys::PrivateKey::from_openssh(TEST_HOST_KEY).expect("解析测试主机密钥")],
        ..Default::default()
    });
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("绑定回环端口");
    let port = listener.local_addr().expect("读取端口").port();
    // RunningServer 借用 server 实例与 listener：整体 move 进后台 future，
    // 由 future 持有至进程结束（测试进程退出自然回收）。
    let events: ServerEvents = Default::default();
    let tmux_sessions = TmuxSessions::default();
    let mut loop_server = LoopServer {
        behavior,
        events: events.clone(),
        tmux_sessions: tmux_sessions.clone(),
    };
    tokio::spawn(async move {
        let server = loop_server.run_on_socket(config, &listener);
        let _ = server.await;
    });
    (port, events, tmux_sessions)
}

// ---------------------------------------------------------------------------
// Fixture（内存库 + 指向回环端口的服务器配置）
// ---------------------------------------------------------------------------

pub(super) struct Fixture {
    pub(super) registry: TermSessionRegistry,
    pub(super) manager: crate::ssh_tool::SshSessionManager,
    pub(super) events: ServerEvents,
    pub(super) tmux_sessions: TmuxSessions,
    _guard: crate::test_util::TempDirGuard,
}

pub(super) async fn fixture(tmux_available: bool) -> Fixture {
    fixture_with(ServerBehavior {
        tmux_available,
        ..Default::default()
    })
    .await
}

pub(super) async fn fixture_with(behavior: ServerBehavior) -> Fixture {
    let (port, events, tmux_sessions) = spawn_loop_server(behavior).await;
    let guard = crate::test_util::TempDirGuard::new("ssh-term-it");
    let pool = Arc::new(
        r2d2::Pool::builder()
            .max_size(1)
            .build(r2d2_sqlite::SqliteConnectionManager::memory())
            .unwrap(),
    );
    {
        let mut connection = pool.get().unwrap();
        let tx = connection.transaction().unwrap();
        crate::ssh_tool::db::ensure_ssh_tables_tx(&tx).unwrap();
        tx.commit().unwrap();
    }
    let ssh_db = SshDb::new(pool.clone());
    ssh_db
        .save_servers(&[serde_json::from_value(json!({
            "id": "loop-server", "host": "127.0.0.1", "port": port,
            "username": "tester", "password": "loop", "reviewEnabled": false
        }))
        .unwrap()])
        .unwrap();
    Fixture {
        registry: TermSessionRegistry::new(),
        manager: crate::ssh_tool::SshSessionManager::new(pool),
        events,
        tmux_sessions,
        _guard: guard,
    }
}

pub(super) fn open_params() -> TermOpenParams {
    TermOpenParams {
        server_id: "loop-server".to_string(),
        session_id: "it-session".to_string(),
        cols: 80,
        rows: 24,
        command: None,
        tmux: TmuxPreference::Auto,
        tmux_session: None,
    }
}

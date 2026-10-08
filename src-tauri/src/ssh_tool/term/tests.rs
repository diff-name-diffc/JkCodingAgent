//! ssh_term_* 集成测试：russh 回环 server 起真实 PTY 通道，覆盖
//! open（shell / exec / tmux 分路）→ send → read → close 全链路。
//!
//! 服务器行为模拟（非真实 shell）：shell 模式回显输入并在 `\r` 后补提示符；
//! exec 模式按命令名脚本化应答（`command -v tmux` 探测 / tmux attach / 普通命令）。

use std::sync::Arc;

use russh::server::{Auth, Msg, Server as _};
use russh::ChannelId;
use serde_json::json;
use tokio::net::TcpListener;

use super::registry::{TermOpenParams, TermSessionRegistry, TmuxPreference};
use crate::ssh_tool::SshDb;

// ---------------------------------------------------------------------------
// 回环服务器
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct LoopServer {
    /// `command -v tmux` 的模拟结果（tmux 分路开关）。
    tmux_available: bool,
}

impl russh::server::Server for LoopServer {
    type Handler = LoopHandler;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self::Handler {
        LoopHandler {
            tmux_available: self.tmux_available,
        }
    }
}

struct LoopHandler {
    tmux_available: bool,
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
        session.data(channel, b"$ ".to_vec())?;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).to_string();
        if command == "command -v tmux" {
            if self.tmux_available {
                session.data(channel, b"/usr/bin/tmux\r\n".to_vec())?;
                session.exit_status_request(channel, 0)?;
            } else {
                session.exit_status_request(channel, 1)?;
            }
            session.eof(channel)?;
            session.close(channel)?;
        } else if command.starts_with("tmux new -A -s ") {
            // 模拟 tmux client：attach 成功，保持通道（不退出）。
            session.data(channel, b"[tmux attached]\r\n".to_vec())?;
        } else {
            session.data(channel, format!("run: {command}\r\n").into_bytes())?;
            session.exit_status_request(channel, 0)?;
            session.eof(channel)?;
            session.close(channel)?;
        }
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        // 行回显 + 回车后的新提示符（模拟登录 shell 的最小行为）。
        session.data(channel, data.to_vec())?;
        if data.contains(&b'\r') {
            session.data(channel, b"\r\n$ ".to_vec())?;
        }
        Ok(())
    }
}

/// 固定测试密钥（仅测试用途，无敏感性）：随机密钥需要 rand 版本与 ssh-key 的
/// rand_core trait 对齐，固定 PEM 避开依赖纠缠。
const TEST_HOST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACDu+QkbiQ5z3ZCGBDxaVo7ZrpBlccoKej8vTkTuo/3hmQAAAKAcisBNHIrA\nTQAAAAtzc2gtZWQyNTUxOQAAACDu+QkbiQ5z3ZCGBDxaVo7ZrpBlccoKej8vTkTuo/3hmQ\nAAAED9lkd8MRooMXd6QPfjYxgEdtIJodhCcWZvfIlRACLsku75CRuJDnPdkIYEPFpWjtmu\nkGVxygp6Py9ORO6j/eGZAAAAGmprQGprcy1NYWNCb29rLVByby01LmxvY2FsAQID\n-----END OPENSSH PRIVATE KEY-----";

async fn spawn_loop_server(tmux_available: bool) -> u16 {
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
    let mut loop_server = LoopServer { tmux_available };
    tokio::spawn(async move {
        let server = loop_server.run_on_socket(config, &listener);
        let _ = server.await;
    });
    port
}

// ---------------------------------------------------------------------------
// Fixture（内存库 + 指向回环端口的服务器配置）
// ---------------------------------------------------------------------------

struct Fixture {
    registry: TermSessionRegistry,
    manager: crate::ssh_tool::SshSessionManager,
    _guard: crate::test_util::TempDirGuard,
}

async fn fixture(tmux_available: bool) -> Fixture {
    let port = spawn_loop_server(tmux_available).await;
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
        _guard: guard,
    }
}

fn open_params() -> TermOpenParams {
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

// ---------------------------------------------------------------------------
// 用例
// ---------------------------------------------------------------------------

#[tokio::test]
async fn open_shell_reads_prompt_without_tmux() {
    let fx = fixture(false).await;
    let payload = fx
        .registry
        .open(&fx.manager, open_params())
        .await
        .expect("open");
    assert!(
        payload.tmux_session.is_none(),
        "tmux 探测失败应回退裸 shell"
    );
    assert!(
        payload.screen.contains('$'),
        "首屏应含提示符：{:?}",
        payload.screen
    );
    assert!(!payload.exited);
    // 收尾
    let _ = fx.registry.close(&payload.term_id, false).await;
}

#[tokio::test]
async fn send_read_roundtrip_finalizes_echo_lines() {
    let fx = fixture(false).await;
    let opened = fx
        .registry
        .open(&fx.manager, open_params())
        .await
        .expect("open");
    // open 的首屏等待已消费提示符帧，先排空再发送。
    let _ = fx.registry.read(&opened.term_id, 0, None).await;
    fx.registry
        .send(&opened.term_id, "echo hi\r")
        .await
        .expect("send");
    let read = fx
        .registry
        .read(&opened.term_id, 3_000, None)
        .await
        .expect("read");
    // 回显行进入增量轨（光标行定稿）。
    assert!(
        read.new_lines.iter().any(|line| line.contains("echo hi")),
        "增量轨应含命令回显：{:?}",
        read.new_lines
    );
    assert!(read.screen.contains('$'), "屏幕应回到提示符");
    let _ = fx.registry.close(&opened.term_id, false).await;
}

#[tokio::test]
async fn open_with_command_runs_exec_path_and_exits() {
    let fx = fixture(false).await;
    let mut params = open_params();
    params.command = Some("htop".to_string());
    let payload = fx.registry.open(&fx.manager, params).await.expect("open");
    assert!(
        payload.screen.contains("run: htop"),
        "exec 输出应上屏：{:?}",
        payload.screen
    );
    // 回环服务器 exec 完成即上报 exit 0 + eof：等 reader 收口。
    let read = fx
        .registry
        .read(&payload.term_id, 2_000, None)
        .await
        .expect("read");
    assert!(read.exited, "命令结束应标记 exited");
    assert_eq!(read.exit_code, Some(0));
    // 再发送应被拒绝。
    let err = fx
        .registry
        .send(&payload.term_id, "x")
        .await
        .expect_err("exited send");
    assert!(matches!(err, super::registry::TermError::Exited(_)));
}

#[tokio::test]
async fn tmux_auto_attach_or_create() {
    let fx = fixture(true).await;
    let payload = fx
        .registry
        .open(&fx.manager, open_params())
        .await
        .expect("open");
    let name = payload.tmux_session.as_deref().expect("应进入 tmux 会话");
    assert!(name.starts_with("jkagent-"), "tmux 会话名前缀：{name}");
    assert!(
        payload.screen.contains("[tmux attached]"),
        "tmux attach 输出应上屏：{:?}",
        payload.screen
    );
    // 同名 open 恢复现场（attach-or-create 语义）。
    let mut params = open_params();
    params.tmux_session = Some(name.to_string());
    let reopened = fx.registry.open(&fx.manager, params).await.expect("reopen");
    assert_eq!(reopened.tmux_session.as_deref(), Some(name));
    let _ = fx.registry.close(&payload.term_id, false).await;
    let _ = fx.registry.close(&reopened.term_id, false).await;
}

#[tokio::test]
async fn close_is_idempotent_and_reports_exit() {
    let fx = fixture(false).await;
    let opened = fx
        .registry
        .open(&fx.manager, open_params())
        .await
        .expect("open");
    let closed = fx
        .registry
        .close(&opened.term_id, false)
        .await
        .expect("close");
    assert_eq!(closed.term_id, opened.term_id);
    // 幂等：再次 close 报 NotFound。
    let err = fx
        .registry
        .close(&opened.term_id, false)
        .await
        .expect_err("double close");
    assert!(matches!(err, super::registry::TermError::NotFound(_)));
    assert!(fx.registry.list(None).is_empty());
}

#[tokio::test]
async fn custom_tmux_name_validated_and_quota_reports() {
    let fx = fixture(true).await;
    // 非法名（缺前缀）在 open 即被拒。
    let mut params = open_params();
    params.tmux_session = Some("evil; rm -rf".to_string());
    let err = fx
        .registry
        .open(&fx.manager, params)
        .await
        .expect_err("非法 tmux 会话名");
    assert!(matches!(err, super::registry::TermError::Open(_)));

    // 每服务器配额 ≤4：开满后第 5 个被拒且报错附清单。
    let mut ids = Vec::new();
    for _ in 0..4 {
        let payload = fx
            .registry
            .open(&fx.manager, open_params())
            .await
            .expect("open quota");
        ids.push(payload.term_id);
    }
    let err = fx
        .registry
        .open(&fx.manager, open_params())
        .await
        .expect_err("超配额");
    match err {
        super::registry::TermError::QuotaExceeded(message) => {
            assert!(
                message.contains("loop-server"),
                "报错应附会话清单：{message}"
            );
        }
        other => panic!("期望配额错误，实际 {other:?}"),
    }
    for id in ids {
        let _ = fx.registry.close(&id, false).await;
    }
}

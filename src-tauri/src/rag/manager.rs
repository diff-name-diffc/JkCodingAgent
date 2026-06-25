//! RAG sidecar 进程管理器。
//!
//! 使用 Tauri 2.0 官方 sidecar 机制（`bundle.externalBin` + `app.shell().sidecar()`）：
//! - 宿主在 `tauri.conf.json` 声明 `binaries/rag-server`
//! - Tauri 运行时自动按 target-triple 解析实际二进制路径
//! - 通过 `tauri-plugin-shell` 启动，无需手动拼路径
//!
//! 启动握手：spawn 后逐行读取 stdout，匹配 `RAG_LISTENING {...}` 取端口；
//! 匹配到后停止解析 stdout（避免持有 reader 任务阻塞），后续日志仅透传 stderr。

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use parking_lot::Mutex;
use serde::Deserialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_shell::process::CommandChild;
use tauri_plugin_shell::ShellExt;
use tokio::sync::oneshot;
use tokio::time::timeout;

use super::config::RagConfigStore;
use super::logs::{RagLogStore, RagLogStream};
use super::transport::RagTransport;

/// sidecar 二进制名（须与 tauri.conf.json `bundle.externalBin` 一致，
/// 也是 PyInstaller 产物名，见 rag/pyproject.toml `[tool.rag-server].binary_name`）。
pub const SIDECAR_NAME: &str = "rag-server";

/// 握手协议前缀，与 `rag/src/rag_server/main.py::_emit_handshake` 对应。
const HANDSHAKE_PREFIX: &str = "RAG_LISTENING";

/// 等待握手的最长时间。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);

/// 已运行 sidecar 的句柄。
pub struct RagHandle {
    pub port: u16,
    pub transport: RagTransport,
    child: Mutex<Option<CommandChild>>,
}

impl RagHandle {
    /// 终止 sidecar 子进程。
    pub fn kill(&self) {
        if let Some(child) = self.child.lock().take() {
            let _ = child.kill();
        }
    }
}

/// 进程级 sidecar 管理器，由 Tauri State 托管。
#[derive(Default)]
pub struct RagManager {
    handle: Mutex<Option<Arc<RagHandle>>>,
}

impl RagManager {
    /// 启动 sidecar 并完成端口握手。若已在运行则直接返回现有句柄。
    pub async fn ensure_started(
        &self,
        app: &AppHandle,
        config_store: &RagConfigStore,
    ) -> Result<Arc<RagHandle>> {
        // 锁外检查现有句柄
        {
            let guard = self.handle.lock();
            if let Some(existing) = guard.as_ref() {
                return Ok(Arc::clone(existing));
            }
        }
        let handle = spawn_and_handshake(app, config_store).await?;
        *self.handle.lock() = Some(Arc::clone(&handle));
        Ok(handle)
    }

    /// 停止 sidecar。
    pub fn stop(&self) {
        if let Some(handle) = self.handle.lock().take() {
            handle.kill();
        }
    }

    /// 当前是否在运行。
    pub fn is_running(&self) -> bool {
        self.handle.lock().is_some()
    }

    /// 取当前句柄（若已启动）。
    pub fn current(&self) -> Option<Arc<RagHandle>> {
        self.handle.lock().as_ref().map(Arc::clone)
    }
}

async fn spawn_and_handshake(
    app: &AppHandle,
    config_store: &RagConfigStore,
) -> Result<Arc<RagHandle>> {
    let config = config_store.get_or_load()?;
    app.state::<RagLogStore>()
        .append_system(app, "正在启动 RAG sidecar");

    // 构造 sidecar 命令；通过 env 注入初始配置
    let mut command = app.shell().sidecar(SIDECAR_NAME).with_context(|| {
        format!(
            "解析 sidecar `{SIDECAR_NAME}` 失败：请先运行 rag/scripts/build_sidecar.* 生成 {}",
            triple_hint()
        )
    })?;

    for (key, value) in config.to_env_pairs() {
        command = command.env(key, value);
    }
    // 固定端口便于骨架阶段握手；生产可改 0 让 OS 分配，sidecar 回传真实端口
    command = command.env("RAG_PORT", "0");

    let (mut rx, child) = command.spawn().context("启动 rag-server sidecar 失败")?;

    // 子进程句柄用 Arc<Mutex<Option<_>>> 共享：超时闭包可 kill，
    // 握手成功后句柄转入 RagHandle 持有。
    let child_slot = Arc::new(Mutex::new(Some(child)));

    let (port_tx, port_rx) = oneshot::channel::<u16>();

    // stdout reader：匹配握手行，拿到端口后即通知
    let app_for_log = app.clone();
    tokio::spawn(async move {
        let mut port_tx = Some(port_tx);
        while let Some(event) = rx.recv().await {
            match event {
                tauri_plugin_shell::process::CommandEvent::Stdout(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    for line in text.lines() {
                        if let Some(rest) = line.trim_start().strip_prefix(HANDSHAKE_PREFIX) {
                            if let Ok(handshake) =
                                serde_json::from_str::<HandshakePayload>(rest.trim())
                            {
                                app_for_log.state::<RagLogStore>().append_system(
                                    &app_for_log,
                                    format!("RAG sidecar 已监听端口 {}", handshake.port),
                                );
                                if let Some(sender) = port_tx.take() {
                                    let _ = sender.send(handshake.port);
                                }
                            }
                        } else {
                            app_for_log.state::<RagLogStore>().append(
                                &app_for_log,
                                RagLogStream::Stdout,
                                line,
                            );
                        }
                    }
                }
                tauri_plugin_shell::process::CommandEvent::Stderr(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    for line in text.lines() {
                        app_for_log.state::<RagLogStore>().append(
                            &app_for_log,
                            RagLogStream::Stderr,
                            line,
                        );
                    }
                }
                tauri_plugin_shell::process::CommandEvent::Terminated(payload) => {
                    app_for_log
                        .state::<RagLogStore>()
                        .append_system(&app_for_log, format!("RAG sidecar 已退出：{payload:?}"));
                    break;
                }
                _ => {}
            }
        }
    });

    let child_slot_for_timeout = Arc::clone(&child_slot);
    let port = timeout(HANDSHAKE_TIMEOUT, port_rx)
        .await
        .map_err(|_| {
            // 超时则回收子进程
            if let Some(child) = child_slot_for_timeout.lock().take() {
                let _ = child.kill();
            }
            app.state::<RagLogStore>().append_system(
                app,
                format!("等待 RAG sidecar 端口握手超时（{HANDSHAKE_TIMEOUT:?}）"),
            );
            anyhow!("等待 rag-server 端口握手超时（{HANDSHAKE_TIMEOUT:?}）")
        })?
        .map_err(|_| anyhow!("rag-server sidecar 在端口握手前已退出，请查看 RAG sidecar 日志"))?;

    let transport = RagTransport::new(port).context("构造 sidecar HTTP client")?;

    // 握手成功，从共享槽位取出子进程句柄转入 RagHandle
    let child = child_slot
        .lock()
        .take()
        .ok_or_else(|| anyhow!("sidecar 子进程句柄已丢失"))?;

    Ok(Arc::new(RagHandle {
        port,
        transport,
        child: Mutex::new(Some(child)),
    }))
}

/// 仅供错误提示用的当前平台 triple 文件名提示。
fn triple_hint() -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    match (os, arch) {
        ("macos", "aarch64") => "rag-server-aarch64-apple-darwin".into(),
        ("macos", "x86_64") => "rag-server-x86_64-apple-darwin".into(),
        ("windows", "x86_64") => "rag-server-x86_64-pc-windows-msvc.exe".into(),
        ("linux", "x86_64") => "rag-server-x86_64-unknown-linux-gnu".into(),
        _ => format!("rag-server-{arch}-{os}"),
    }
}

#[derive(Deserialize)]
struct HandshakePayload {
    port: u16,
}

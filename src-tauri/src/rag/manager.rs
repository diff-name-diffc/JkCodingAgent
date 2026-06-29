//! RAG sidecar 进程管理器。
//!
//! 使用 Tauri 2.0 官方 sidecar 机制（`bundle.externalBin` + `app.shell().sidecar()`）：
//! - 宿主在 `tauri.conf.json` 声明 `binaries/rag-server`
//! - Tauri 运行时自动按 target-triple 解析实际二进制路径
//! - 通过 `tauri-plugin-shell` 启动，无需手动拼路径
//!
//! 启动握手：spawn 后逐行读取 stdout，匹配 `RAG_LISTENING {...}` 取端口；
//! 匹配到后停止解析 stdout（避免持有 reader 任务阻塞），后续日志仅透传 stderr。
//!
//! 生命周期约束（杜绝孤儿 / 僵尸子进程）：
//! - 所有 spawn / stop / restart 串行经过 `spawn_lock`（tokio 异步互斥），
//!   确保「检查存活 → spawn → 写入句柄」整体不可分割，消除 TOCTOU 竞态。
//! - `RagHandle` 自带 `alive` 标志与退出 watch：reader 收到 `Terminated` 即标记死亡，
//!   `is_running` / `current` 据此过滤，崩溃后下次 `ensure_started` 自动重建。
//! - `stop` 在 kill 后等待子进程真正退出（带超时），避免端口/资源未释放就重启。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use parking_lot::Mutex;
use serde::Deserialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_shell::process::CommandChild;
use tauri_plugin_shell::ShellExt;
use tokio::sync::{oneshot, watch, Mutex as AsyncMutex};
use tokio::time::{sleep, timeout};

use super::config::RagConfigStore;
use super::logs::{RagLogStore, RagLogStream};
use super::transport::RagTransport;

/// sidecar 二进制名（须与 tauri.conf.json `bundle.externalBin` 一致，
/// 也是 PyInstaller 产物名，见 rag/pyproject.toml `[tool.rag-server].binary_name`）。
pub const SIDECAR_NAME: &str = "rag-server";

/// 握手协议前缀，与 `rag/src/rag_server/main.py::_emit_handshake` 对应。
const HANDSHAKE_PREFIX: &str = "RAG_LISTENING";

/// 等待握手的最长时间。
///
/// PyInstaller onefile + RAG 依赖冷启动可能明显超过 20s，过早 kill 会把“慢启动”
/// 误判成“启动失败”。这里等待进程真正给出端口，再由健康检查确认 HTTP 可用。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(90);

/// 收到端口握手后，等待 HTTP 服务真正可用的最长时间。
const HEALTH_READY_TIMEOUT: Duration = Duration::from_secs(20);

/// kill 子进程后等待其真正退出的最长时间。
/// 超过则不再阻塞调用方——进程退出时 OS 仍会回收，但端口可能尚未释放。
const EXIT_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

/// 已运行 sidecar 的句柄。
pub struct RagHandle {
    pub port: u16,
    pub transport: RagTransport,
    child: Mutex<Option<CommandChild>>,
    /// 子进程是否仍存活。reader 收到 `Terminated` 后置 false，
    /// `is_running` / `current` / `ensure_started` 据此判断是否需重建。
    alive: Arc<AtomicBool>,
    /// 退出信号：reader 在 `Terminated` 时 send(true)，`wait_exit` 据此等待。
    exited: watch::Receiver<bool>,
}

impl RagHandle {
    /// 终止 sidecar 子进程（仅发 kill 信号，不等退出）。
    pub fn kill(&self) {
        if let Some(child) = self.child.lock().take() {
            let _ = child.kill();
        }
    }

    /// 子进程是否仍存活。
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    /// kill 后等待 reader 报告 `Terminated`，确保进程退出、端口释放。
    /// 若已死亡则立即返回；超过 `EXIT_WAIT_TIMEOUT` 也不再阻塞。
    pub async fn wait_exit(&self) {
        if !self.is_alive() {
            return;
        }
        let mut rx = self.exited.clone();
        let _ = timeout(EXIT_WAIT_TIMEOUT, async {
            let _ = rx.wait_for(|v| *v).await;
        })
        .await;
    }
}

/// 进程级 sidecar 管理器，由 Tauri State 托管。
pub struct RagManager {
    /// 串行化所有 spawn / stop / restart：持锁期间不允许另一个 spawn 写入句柄，
    /// 根除「并发 ensure_started 各 spawn 一个、后者覆盖前者致孤儿」的竞态。
    /// 用 tokio 异步互斥——临界区跨 spawn + 握手等异步 I/O，不阻塞 runtime 线程。
    spawn_lock: AsyncMutex<()>,
    handle: Mutex<Option<Arc<RagHandle>>>,
}

impl Default for RagManager {
    fn default() -> Self {
        Self {
            spawn_lock: AsyncMutex::new(()),
            handle: Mutex::new(None),
        }
    }
}

impl RagManager {
    /// 启动 sidecar 并完成端口握手。若已在运行（且存活）则直接返回现有句柄；
    /// 若句柄存在但子进程已崩溃，则先回收旧句柄再 spawn 新的。
    pub async fn ensure_started(
        &self,
        app: &AppHandle,
        config_store: &RagConfigStore,
    ) -> Result<Arc<RagHandle>> {
        let _guard = self.spawn_lock.lock().await;
        // 锁内复查存活状态——锁外检查无意义（释放锁瞬间即可改变）。
        // 临界区内只做内存读写并取出需要 wait 的句柄，不跨 await 持有 parking_lot 守卫
        // （其非 Send，会让命令 future 失去 Send）。
        let dead = {
            let guard = self.handle.lock();
            match guard.as_ref() {
                Some(existing) if existing.is_alive() => return Ok(Arc::clone(existing)),
                _ => guard.as_ref().map(Arc::clone),
            }
        };
        // 句柄尚在但子进程已死：回收资源（kill 兜底 + 等退出）再 spawn 新的。
        if let Some(d) = dead {
            d.kill();
            d.wait_exit().await;
        }
        let handle = spawn_and_handshake(app, config_store).await?;
        *self.handle.lock() = Some(Arc::clone(&handle));
        Ok(handle)
    }

    /// 原子重启：在同一把 spawn 锁内 stop + spawn，避免两步之间插入其他调用。
    pub async fn restart(
        &self,
        app: &AppHandle,
        config_store: &RagConfigStore,
    ) -> Result<Arc<RagHandle>> {
        let _guard = self.spawn_lock.lock().await;
        self.stop_locked().await;
        let handle = spawn_and_handshake(app, config_store).await?;
        *self.handle.lock() = Some(Arc::clone(&handle));
        Ok(handle)
    }

    /// 停止 sidecar：kill 后等待子进程真正退出，确保端口释放。
    pub async fn stop(&self) {
        let _guard = self.spawn_lock.lock().await;
        self.stop_locked().await;
    }

    /// 应用退出路径的同步停止：只 take + kill，不等退出。
    /// 主进程即将退出，OS 会回收子进程；同步上下文也无法 await。
    pub fn stop_for_exit(&self) {
        if let Some(handle) = self.handle.lock().take() {
            handle.kill();
        }
    }

    /// 锁内停止实现：调用方已持有 `spawn_lock`。
    /// 先 take 出句柄释放 parking_lot 锁，再 await wait_exit，避免守卫跨 await。
    async fn stop_locked(&self) {
        let handle = self.handle.lock().take();
        if let Some(handle) = handle {
            handle.kill();
            handle.wait_exit().await;
        }
    }

    /// 当前是否在运行（句柄存在且子进程存活）。
    pub fn is_running(&self) -> bool {
        self.handle.lock().as_ref().is_some_and(|h| h.is_alive())
    }

    /// 取当前存活句柄（若已启动且未崩溃）。死句柄返回 None，促使调用方重启。
    pub fn current(&self) -> Option<Arc<RagHandle>> {
        self.handle
            .lock()
            .as_ref()
            .filter(|h| h.is_alive())
            .map(Arc::clone)
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

    // 存活标志 + 退出 watch：reader 收到 Terminated 时翻转 alive 并通知 wait_exit。
    let alive = Arc::new(AtomicBool::new(true));
    let alive_for_timeout = Arc::clone(&alive);
    let (exited_tx, exited_rx) = watch::channel(false);
    let exited_tx_for_timeout = exited_tx.clone();

    // stdout reader：匹配握手行，拿到端口后即通知
    let app_for_log = app.clone();
    let alive_for_reader = Arc::clone(&alive);
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
                    mark_dead(&alive_for_reader, &exited_tx);
                    app_for_log
                        .state::<RagLogStore>()
                        .append_system(&app_for_log, format!("RAG sidecar 已退出：{payload:?}"));
                    break;
                }
                _ => {}
            }
        }
        // reader 结束兜底：管道关闭但未收到 Terminated 时，同样标记死亡，
        // 避免 wait_exit 永久挂起、is_running 误报存活。
        mark_dead(&alive_for_reader, &exited_tx);
    });

    let child_slot_for_timeout = Arc::clone(&child_slot);
    let port = timeout(HANDSHAKE_TIMEOUT, port_rx)
        .await
        .map_err(|_| {
            // 超时则回收子进程
            if let Some(child) = child_slot_for_timeout.lock().take() {
                let _ = child.kill();
            }
            mark_dead(&alive_for_timeout, &exited_tx_for_timeout);
            app.state::<RagLogStore>().append_system(
                app,
                format!("等待 RAG sidecar 端口握手超时（{HANDSHAKE_TIMEOUT:?}）"),
            );
            anyhow!("等待 rag-server 端口握手超时（{HANDSHAKE_TIMEOUT:?}）")
        })?
        .map_err(|_| {
            // 握手通道在收到端口前关闭——子进程已退出（reader 已标记死亡）。
            mark_dead(&alive_for_timeout, &exited_tx_for_timeout);
            anyhow!("rag-server sidecar 在端口握手前已退出，请查看 RAG sidecar 日志")
        })?;

    let transport = RagTransport::new(port).context("构造 sidecar HTTP client")?;
    if let Err(error) = wait_for_health(app, &transport).await {
        if let Some(child) = child_slot.lock().take() {
            let _ = child.kill();
        }
        mark_dead(&alive_for_timeout, &exited_tx_for_timeout);
        return Err(error);
    }

    // 握手成功，从共享槽位取出子进程句柄转入 RagHandle
    let child = child_slot
        .lock()
        .take()
        .ok_or_else(|| anyhow!("sidecar 子进程句柄已丢失"))?;

    Ok(Arc::new(RagHandle {
        port,
        transport,
        child: Mutex::new(Some(child)),
        alive,
        exited: exited_rx,
    }))
}

async fn wait_for_health(app: &AppHandle, transport: &RagTransport) -> Result<()> {
    let start = std::time::Instant::now();
    let mut last_error: Option<anyhow::Error> = None;

    while start.elapsed() < HEALTH_READY_TIMEOUT {
        match transport.health().await {
            Ok(_) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                sleep(Duration::from_millis(250)).await;
            }
        }
    }

    app.state::<RagLogStore>().append_system(
        app,
        format!("RAG sidecar 端口已握手，但健康检查超时（{HEALTH_READY_TIMEOUT:?}）"),
    );
    Err(anyhow!(
        "rag-server 端口已握手但 HTTP 服务未就绪（{HEALTH_READY_TIMEOUT:?}）：{}",
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "未收到健康检查响应".to_string())
    ))
}

/// 标记句柄死亡：翻转 alive 并通知所有 wait_exit 等待者。
fn mark_dead(alive: &AtomicBool, exited_tx: &watch::Sender<bool>) {
    alive.store(false, Ordering::Release);
    let _ = exited_tx.send(true);
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

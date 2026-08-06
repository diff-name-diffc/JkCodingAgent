//! RAG sidecar 进程管理器。
//!
//! **不使用 tauri-plugin-shell 的 sidecar**——其 `Command` API 不暴露
//! `process_group`，无法整组 kill。这里直接用 `tokio::process::Command`：
//! - spawn 时 `.process_group(0)` 让 rag-server 成为新进程组组长（pgid == pid）
//! - kill 时整组终止：Unix `kill(-pgid, SIGKILL)`、Windows `taskkill /F /T /PID`
//!
//! 这对 PyInstaller onefile 至关重要：onefile 是「bootloader → Python 子进程」
//! 两层结构，只 kill bootloader（直接子进程）会让 Python 子进程逃逸成孤儿
//! （PPID=1），这正是历史上泄漏十几个 rag-server 的根因。整组 kill 才能连带
//! 终止 Python 服务进程。
//!
//! 启动握手：spawn 后逐行读取 stdout，匹配 `RAG_LISTENING {...}` 取端口；
//! 后续 stdout/stderr 仅透传到滚动日志。
//!
//! 生命周期约束（杜绝孤儿 / 僵尸子进程）：
//! - 所有 spawn / stop / restart 串行经过 `spawn_lock`（tokio 异步互斥），
//!   确保「检查存活 → spawn → 写入句柄」整体不可分割，消除 TOCTOU 竞态。
//! - `RagHandle` 自带 `alive` 标志与退出 watch：stdout reader、stderr reader、
//!   wait reaper 三个后台任务任一发现进程退出即标记死亡，
//!   `is_running` / `current` 据此过滤，崩溃后下次 `ensure_started` 自动重建。
//! - wait reaper 任务独占子进程句柄并调用 `wait()` 回收 bootloader 避免 zombie；
//!   Python 子进程随进程组被 kill，被 init/launchd 回收。
//! - `stop` 在 kill 后等待退出信号（带超时），避免端口/资源未释放就重启。
//! - 应用退出路径（`stop_for_exit`）与 fork 点通过同步 `exit_guard` 互斥：
//!   退出时先置 `shutting_down` 再回收已登记句柄/在途进程组，spawn 侧在
//!   fork 前检查标志、fork 成功立即登记 pgid——二者串行，保证
//!   「退出判定之后绝不 fork，fork 之后必被退出路径看见」。
//! - 宿主以管道 stdin spawn sidecar 并由 `RagHandle` 持有写端；宿主无论
//!   正常退出还是被 kill -9 / 崩溃，管道都会关闭，sidecar 侧监视 stdin EOF
//!   后自杀（见 rag_server/main.py 的宿主生命周期监视线程），兜底所有
//!   应用内回调无法触发的强杀场景。

use std::path::PathBuf;
#[cfg(windows)]
use std::process::Command as StdCommand;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use parking_lot::Mutex;
use serde::Deserialize;
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{ChildStdin, Command};
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
    /// 进程组 ID。Unix 上 `.process_group(0)` 使子进程成为组长，pgid == pid；
    /// kill 时对负数 pgid 发信号即可整组终止。Windows 上存 pid 供 `taskkill /T`。
    pgid: i32,
    /// 子进程是否仍存活。任一 reader/reaper 发现退出后置 false，
    /// `is_running` / `current` / `ensure_started` 据此判断是否需重建。
    alive: Arc<AtomicBool>,
    /// 退出信号：reaper/reader 在进程退出时 send(true)，`wait_exit` 据此等待。
    exited: watch::Receiver<bool>,
    /// 宿主与 sidecar 之间的「生命周期脐带」：spawn 时以管道 stdin 连接，
    /// 这里只持有写端、永不写入。句柄被 drop（应用退出 / stop / restart）
    /// 或宿主进程整个消失（kill -9 / 崩溃，内核关闭全部 fd）时管道关闭，
    /// sidecar 的监视线程读到 EOF 后自行退出，保证 sidecar 不比宿主长寿。
    #[allow(dead_code)]
    stdin_guard: Option<ChildStdin>,
}

impl RagHandle {
    /// 终止 sidecar 整个进程组（仅发信号，不等退出——退出等待由 `wait_exit` 负责）。
    pub fn kill(&self) {
        kill_process_group(self.pgid);
    }

    /// 子进程是否仍存活。
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    /// kill 后等待 reaper/reader 报告退出，确保进程退出、端口释放。
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

/// 整组终止 sidecar 进程。
///
/// - Unix：`kill(-pgid, SIGKILL)` 对整个进程组发信号，连带 PyInstaller 派生
///   的 Python 子进程，杜绝 onefile 孤儿。
/// - Windows：`taskkill /F /T /PID` 递归杀进程树（`/T`）达成同样效果。
fn kill_process_group(pgid: i32) {
    #[cfg(unix)]
    {
        // 负数 pid = 信号发送给「pid 绝对值所标识进程组」的全部成员。
        // ESRCH（组已不存在）忽略即可。
        let _ = unsafe { libc::kill(-pgid, libc::SIGKILL) };
    }
    #[cfg(windows)]
    {
        // /F 强制、/T 连同子进程树一起终止。
        let _ = StdCommand::new("taskkill")
            .args(["/F", "/T", "/PID"])
            .arg(pgid.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// fork 点与应用退出路径的同步互斥状态。
///
/// 临界区只覆盖「标志检查 + fork + 登记 pgid」这几条内存/系统调用，
/// 不跨任何 await，保证退出回调（同步上下文）也能进入。
struct ExitGuard {
    /// 应用已决定退出。置位后所有新的 spawn 必须放弃。
    shutting_down: bool,
    /// 已 fork 但尚未写入 `handle` 的在途进程组 id（处于握手窗口期）。
    /// 退出路径需要连它一起回收，否则慢启动的 sidecar 会逃逸。
    pending_pgid: Option<i32>,
}

/// RAII 清理守卫：`guarded_spawn` 成功后的任何早退路径（stdout/stderr 捕获失败、
/// transport 构造失败、握手超时/提前退出、健康检查失败等）在 Drop 时自动
/// 「整组 kill + 清除在途 pgid 登记」。此前这些分支需手工逐一调用
/// `kill_process_group`/`clear_pending`，漏掉任何一处就会留下陈旧 pgid 与
/// 无人管理的 sidecar；守卫统一覆盖所有退出分支，消除遗漏面。
///
/// 成功路径调用 `defuse()` 解除；在途登记保留到 `register_handle` 在同一个
/// `exit_guard` 临界区内随句柄登记一起清除（见该方法注释）。
struct SpawnCleanup<'a> {
    manager: &'a RagManager,
    pgid: i32,
}

impl<'a> SpawnCleanup<'a> {
    fn new(manager: &'a RagManager, pgid: i32) -> Self {
        Self { manager, pgid }
    }

    /// 成功路径：进程组所有权移交给 RagHandle，解除清理。
    fn defuse(self) {
        std::mem::forget(self);
    }
}

impl Drop for SpawnCleanup<'_> {
    fn drop(&mut self) {
        kill_process_group(self.pgid);
        self.manager.clear_pending();
    }
}

/// 进程级 sidecar 管理器，由 Tauri State 托管。
pub struct RagManager {
    /// 串行化所有 spawn / stop / restart：持锁期间不允许另一个 spawn 写入句柄，
    /// 根除「并发 ensure_started 各 spawn 一个、后者覆盖前者致孤儿」的竞态。
    /// 用 tokio 异步互斥——临界区跨 spawn + 握手等异步 I/O，不阻塞 runtime 线程。
    spawn_lock: AsyncMutex<()>,
    handle: Mutex<Option<Arc<RagHandle>>>,
    /// fork 点与退出路径的同步守卫（见 `ExitGuard`）。
    exit_guard: Mutex<ExitGuard>,
}

impl Default for RagManager {
    fn default() -> Self {
        Self {
            spawn_lock: AsyncMutex::new(()),
            handle: Mutex::new(None),
            exit_guard: Mutex::new(ExitGuard {
                shutting_down: false,
                pending_pgid: None,
            }),
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
        let handle = self.spawn_and_handshake(app, config_store).await?;
        // 句柄登记与清除在途登记在同一临界区完成（见 register_handle），
        // 消除退出路径的回收竞态窗口。
        self.register_handle(handle)
    }

    /// 原子重启：在同一把 spawn 锁内 stop + spawn，避免两步之间插入其他调用。
    pub async fn restart(
        &self,
        app: &AppHandle,
        config_store: &RagConfigStore,
    ) -> Result<Arc<RagHandle>> {
        let _guard = self.spawn_lock.lock().await;
        self.stop_locked().await;
        let handle = self.spawn_and_handshake(app, config_store).await?;
        self.register_handle(handle)
    }

    /// 停止 sidecar：kill 后等待子进程真正退出，确保端口释放。
    pub async fn stop(&self) {
        let _guard = self.spawn_lock.lock().await;
        self.stop_locked().await;
    }

    /// 应用退出路径的同步停止：置退出标志 + 整组 kill，不等退出。
    /// 主进程即将退出，OS 会回收子进程；同步上下文也无法 await。
    ///
    /// 与 spawn 侧通过 `exit_guard` 串行化：
    /// - 若此刻没有 sidecar 也没有在途 fork：置位 `shutting_down` 后，
    ///   后续任何 spawn 都会在 fork 前看到标志并放弃；
    /// - 若 fork 刚完成、句柄尚未登记（握手窗口期）：守卫里已登记
    ///   `pending_pgid`，这里一并整组 kill；
    /// - 若 spawn 正卡在 fork 系统调用上：`exit_guard` 互斥使本函数
    ///   等到它登记完 pgid 才执行，不会漏杀。
    pub fn stop_for_exit(&self) {
        let pending = {
            let mut guard = self.exit_guard.lock();
            guard.shutting_down = true;
            guard.pending_pgid.take()
        };
        if let Some(pgid) = pending {
            kill_process_group(pgid);
        }
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

impl RagManager {
    /// spawn + 端口握手 + 健康检查（调用方须已持有 `spawn_lock`）。
    /// 拆成自由函数体是为了把「同步守卫临界区」与异步握手分离，见
    /// `guarded_spawn`。
    async fn spawn_and_handshake(
        &self,
        app: &AppHandle,
        config_store: &RagConfigStore,
    ) -> Result<Arc<RagHandle>> {
        spawn_and_handshake_impl(self, app, config_store).await
    }

    /// spawn 与退出路径的同步互斥：检查退出标志 → fork → 登记在途 pgid。
    /// 临界区内无 await / 无阻塞 I/O（fork+exec 本身是毫秒级系统调用）。
    /// 返回 None 表示应用已在退出，调用方必须放弃启动。
    fn guarded_spawn(&self, command: &mut Command) -> Result<Option<tokio::process::Child>> {
        let mut guard = self.exit_guard.lock();
        if guard.shutting_down {
            return Ok(None);
        }
        let child = command.spawn()?;
        // fork 成功即登记进程组：此后任何时刻退出路径都能找到并回收它。
        if let Some(pid) = child.id() {
            guard.pending_pgid = Some(pid as i32);
        }
        Ok(Some(child))
    }

    /// 清除在途 pgid 登记。两条路径调用：
    /// - 失败分支：由 `SpawnCleanup` 守卫在 Drop 时（整组 kill 之后）统一调用；
    /// - 成功路径：由 `register_handle` 与句柄登记在同一临界区内调用。
    ///
    /// 陈旧 pgid 不得滞留——进程组消亡后 pid 可能被系统复用，误杀风险见
    /// `stop_for_exit` 的注释。
    fn clear_pending(&self) {
        self.exit_guard.lock().pending_pgid = None;
    }

    /// 登记握手成功的句柄：复查退出标志、清除在途 pgid、写入 handle 三者在
    /// 同一个 `exit_guard` 临界区内完成。此前「clear_pending 之后、handle 登记
    /// 之前」存在窗口，窗口内触发 `stop_for_exit` 时退出路径既拿不到 pending
    /// 也看不到 handle，sidecar 会逃逸整组 kill。锁顺序 exit_guard → handle，
    /// 与 `stop_for_exit` 一致，不会死锁。
    fn register_handle(&self, handle: Arc<RagHandle>) -> Result<Arc<RagHandle>> {
        let mut guard = self.exit_guard.lock();
        if guard.shutting_down {
            drop(guard);
            handle.kill();
            anyhow::bail!("应用正在退出，放弃登记 RAG sidecar 句柄");
        }
        guard.pending_pgid = None;
        *self.handle.lock() = Some(Arc::clone(&handle));
        Ok(handle)
    }
}

async fn spawn_and_handshake_impl(
    manager: &RagManager,
    app: &AppHandle,
    config_store: &RagConfigStore,
) -> Result<Arc<RagHandle>> {
    let config = config_store.get_or_load()?;
    app.state::<RagLogStore>()
        .append_system(app, "正在启动 RAG sidecar");

    let binary_path = resolve_sidecar_path().with_context(|| {
        format!(
            "解析 sidecar `{SIDECAR_NAME}` 失败：请先运行 rag/scripts/build_sidecar.* 生成 {}",
            triple_hint()
        )
    })?;

    let mut command = Command::new(&binary_path);
    // stdin 用管道连接并由 RagHandle 持有写端，作为「生命周期脐带」：
    // 宿主退出（含被 kill -9 / 崩溃）时内核关闭管道，sidecar 读到 EOF
    // 后自杀。绝不使用 Stdio::null()——那样 sidecar 无从感知宿主死亡。
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    // 握手期早退路径中子进程句柄可能被直接 drop：kill_on_drop 保证 drop 时
    // 终止并回收子进程，与下方清理守卫的整组 kill 互为兜底，不留 zombie。
    command.kill_on_drop(true);
    // 创建新进程组：子进程成为组长（pgid == pid），kill 时用负数 pgid
    // 即可整组终止，连带 PyInstaller 派生的 Python 子进程。
    command.process_group(0);

    // 固定端口便于骨架阶段握手；生产可改 0 让 OS 分配，sidecar 回传真实端口
    command.env("RAG_PORT", "0");
    for (key, value) in config.to_env_pairs() {
        command.env(key, value);
    }

    let mut child = manager
        .guarded_spawn(&mut command)
        .with_context(|| format!("启动 rag-server sidecar 失败：{binary_path:?}"))?
        .ok_or_else(|| anyhow!("应用正在退出，取消 RAG sidecar 启动"))?;
    let pgid = child
        .id()
        .ok_or_else(|| anyhow!("无法获取 rag-server 子进程 PID"))? as i32;
    // RAII 清理守卫：统一覆盖此后所有早退路径（stdout/stderr 捕获失败、
    // transport 构造失败、握手超时/提前退出、健康检查失败）。守卫 Drop 时
    // 自动整组 kill + 清除在途登记，无需各分支手工逐一处理。
    let cleanup = SpawnCleanup::new(manager, pgid);

    let stdin_guard = child.stdin.take();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("rag-server stdout 未捕获"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("rag-server stderr 未捕获"))?;

    // 存活标志 + 退出 watch：reaper/reader 发现进程退出时翻转 alive 并通知 wait_exit。
    let alive = Arc::new(AtomicBool::new(true));
    let alive_for_timeout = Arc::clone(&alive);
    let (exited_tx, exited_rx) = watch::channel(false);
    let exited_tx_for_timeout = exited_tx.clone();

    let (port_tx, port_rx) = oneshot::channel::<u16>();

    // stdout reader：匹配握手行取端口，其余透传日志；EOF 兜底标记死亡。
    let app_for_log = app.clone();
    let alive_for_stdout = Arc::clone(&alive);
    let exited_tx_for_stdout = exited_tx.clone();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout);
        let mut port_tx = Some(port_tx);
        let mut buf = String::new();
        loop {
            buf.clear();
            match reader.read_line(&mut buf).await {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let line = buf.trim_end_matches(['\r', '\n']);
                    if let Some(rest) = line.trim_start().strip_prefix(HANDSHAKE_PREFIX) {
                        if let Ok(handshake) = serde_json::from_str::<HandshakePayload>(rest.trim())
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
                Err(_) => break,
            }
        }
        // 管道关闭兜底：未收到显式退出也标记死亡，避免 wait_exit 挂起、is_running 误报。
        mark_dead(&alive_for_stdout, &exited_tx_for_stdout);
    });

    // stderr reader：仅透传日志；EOF 同样兜底标记死亡。
    let app_for_err = app.clone();
    let alive_for_stderr = Arc::clone(&alive);
    let exited_tx_for_stderr = exited_tx.clone();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        let mut buf = String::new();
        loop {
            buf.clear();
            match reader.read_line(&mut buf).await {
                Ok(0) => break,
                Ok(_) => {
                    let line = buf.trim_end_matches(['\r', '\n']);
                    app_for_err.state::<RagLogStore>().append(
                        &app_for_err,
                        RagLogStream::Stderr,
                        line,
                    );
                }
                Err(_) => break,
            }
        }
        mark_dead(&alive_for_stderr, &exited_tx_for_stderr);
    });

    // wait reaper：独占子进程句柄，wait() 回收 bootloader 避免 zombie；
    // 进程退出后（含整组 kill 触发的退出）标记死亡并通知 wait_exit。
    let alive_for_wait = Arc::clone(&alive);
    let app_for_wait = app.clone();
    tokio::spawn(async move {
        match child.wait().await {
            Ok(status) => {
                mark_dead(&alive_for_wait, &exited_tx);
                app_for_wait
                    .state::<RagLogStore>()
                    .append_system(&app_for_wait, format!("RAG sidecar 已退出：{status}"));
            }
            Err(error) => {
                mark_dead(&alive_for_wait, &exited_tx);
                app_for_wait
                    .state::<RagLogStore>()
                    .append_system(&app_for_wait, format!("RAG sidecar 等待退出失败：{error}"));
            }
        }
    });

    let port = timeout(HANDSHAKE_TIMEOUT, port_rx)
        .await
        .map_err(|_| {
            // 清理守卫 Drop 时整组 kill 并清除在途登记；这里只标记死亡与记日志。
            mark_dead(&alive_for_timeout, &exited_tx_for_timeout);
            app.state::<RagLogStore>().append_system(
                app,
                format!("等待 RAG sidecar 端口握手超时（{HANDSHAKE_TIMEOUT:?}）"),
            );
            anyhow!("等待 rag-server 端口握手超时（{HANDSHAKE_TIMEOUT:?}）")
        })?
        .map_err(|_| {
            // 握手通道在收到端口前关闭——子进程已退出（reader/reaper 已标记死亡）。
            // 对已退出的进程组再 kill 无害（ESRCH 被忽略），统一交给清理守卫。
            mark_dead(&alive_for_timeout, &exited_tx_for_timeout);
            anyhow!("rag-server sidecar 在端口握手前已退出，请查看 RAG sidecar 日志")
        })?;

    let transport = RagTransport::new(port).context("构造 sidecar HTTP client")?;
    if let Err(error) = wait_for_health(app, &transport).await {
        mark_dead(&alive_for_timeout, &exited_tx_for_timeout);
        return Err(error);
    }

    let handle = Arc::new(RagHandle {
        port,
        transport,
        pgid,
        alive,
        exited: exited_rx,
        stdin_guard,
    });
    // 成功路径：解除清理守卫，进程组移交 RagHandle；在途登记由调用方
    // register_handle 随句柄登记在同一临界区清除，退出路径全程可见。
    cleanup.defuse();
    Ok(handle)
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

/// 解析 sidecar 二进制路径：与 tauri-plugin-shell 的相对路径解析一致——
/// 取当前可执行文件所在目录拼接 sidecar 名（Windows 补 `.exe`）。
/// Tauri 构建时已把 `binaries/rag-server-<triple>` 复制为 `rag-server`
/// 放在主程序同级目录，故运行时无需追加 target-triple 后缀。
fn resolve_sidecar_path() -> Result<PathBuf> {
    let exe_path = std::env::current_exe().context("获取当前可执行文件路径失败")?;
    let exe_dir = exe_path
        .parent()
        .ok_or_else(|| anyhow!("当前可执行文件无父目录"))?;
    // 测试场景下 exe 在 deps 子目录，需上一级。
    let base_dir = if exe_dir.ends_with("deps") {
        exe_dir.parent().unwrap_or(exe_dir)
    } else {
        exe_dir
    };
    let path = base_dir.join(SIDECAR_NAME);
    #[cfg(windows)]
    let path = {
        let mut p = path;
        p.set_extension("exe");
        p
    };
    Ok(path)
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

//! local_zsh 命令会话：运行注册表 + 命令监督器（codex shell 的
//! yield/session 模型：软等待窗口 → 转后台继续跑 → 按 run_id 收割）。
//!
//! 一次命令启动后由监督任务全权负责：流式收集输出（有界缓冲）、等待退出 /
//! 硬超时 / 取消、按进程组终止、写审计与命令台账。工具调用侧只在
//! `yield_time_ms` 窗口内等结果：窗口内结束走同步路径渲染；未结束返回
//! run_id 转后台，后续用 `wait_run_id` 继续等待。
//!
//! 硬超时沿用 `exec_timeout_secs`（构造期配置）：后台运行同样受其约束，
//! 到点由监督器终止并落审计——后台不是无限寿命，只是把「等它结束」从
//! 工具调用里解耦。

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::watch;

use super::{LocalZshAuditEntry, MAX_OUTPUT_BYTES};
use crate::agent::command_history::{self, CommandHistoryStatus};

/// 每个会话（workspace）允许的并行后台运行上限：超出拒绝新命令，逼模型
/// 先收割既有运行。
const MAX_ACTIVE_RUNS_PER_SESSION: usize = 8;
/// 注册表里每个会话保留的运行条目上限（含已完成未收割）；超出时淘汰最旧
/// 的已完成条目。
const MAX_TRACKED_RUNS_PER_SESSION: usize = 32;

struct RunState {
    /// 监督器写完审计（含历史回读）并置位完成标记后，本结构整体只读。
    finished: bool,
    exit_code: Option<i32>,
    timed_out: bool,
    cancelled: bool,
    duration_ms: u128,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    total_bytes: usize,
    /// 按原始缓冲长度计算的截断标记（审计同款口径，快照直接取用）。
    output_truncated: bool,
    /// 审计写入成功后的本会话历史（渲染用）；写入失败为 None +
    /// `audit_error` 留痕。
    history: Option<Vec<LocalZshAuditEntry>>,
    audit_error: Option<String>,
}

pub(super) struct BackgroundRun {
    pub(super) run_id: String,
    pub(super) workspace_id: String,
    pub(super) command: String,
    started_at: Instant,
    state: Mutex<RunState>,
    done_tx: watch::Sender<bool>,
    done_rx: watch::Receiver<bool>,
}

/// 等待窗口后的运行快照：仍在跑（含已有输出）或已结束的终态。
pub(super) enum RunSnapshot {
    Running {
        run_id: String,
        elapsed_ms: u128,
        stdout: String,
        stderr: String,
        output_truncated: bool,
    },
    Finished(Box<FinishedRun>),
}

pub(super) struct FinishedRun {
    pub(super) exit_code: Option<i32>,
    pub(super) timed_out: bool,
    pub(super) cancelled: bool,
    pub(super) duration_ms: u128,
    pub(super) stdout: String,
    pub(super) stderr: String,
    /// 监督器按原始缓冲长度算好的截断标记（stdout/stderr 快照已 trim，
    /// 不能再用文本长度回推）。
    pub(super) output_truncated: bool,
    pub(super) history: Vec<LocalZshAuditEntry>,
    pub(super) audit_error: Option<String>,
}

/// 监督器完成命令生命周期所需的全量上下文（审查已在启动前完成，此处只
/// 携带结论落审计）。
pub(super) struct SupervisorContext {
    pub(super) workspace_id: String,
    pub(super) command: String,
    pub(super) run_dir: PathBuf,
    pub(super) audit_dir: PathBuf,
    pub(super) temp_dir: Option<PathBuf>,
    pub(super) review: crate::ssh_tool::SshAuditReview,
    pub(super) exec_timeout_secs: u64,
    pub(super) cancel_rx: Option<watch::Receiver<bool>>,
    /// 普通聊天（绑定工程目录）时命令台账附工作目录前缀，对齐同步路径。
    pub(super) history_with_cwd: bool,
}

/// 启动一条命令并交给监督器：返回注册进注册表的运行句柄。
pub(super) fn start_command(
    context: SupervisorContext,
    tty: bool,
) -> Result<Arc<BackgroundRun>, String> {
    let (done_tx, done_rx) = watch::channel(false);
    let run = Arc::new(BackgroundRun {
        run_id: format!("zsh-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]),
        workspace_id: context.workspace_id.clone(),
        command: context.command.clone(),
        started_at: Instant::now(),
        state: Mutex::new(RunState {
            finished: false,
            exit_code: None,
            timed_out: false,
            cancelled: false,
            duration_ms: 0,
            stdout: Vec::new(),
            stderr: Vec::new(),
            total_bytes: 0,
            output_truncated: false,
            history: None,
            audit_error: None,
        }),
        done_tx,
        done_rx,
    });
    register(&run)?;

    // spawn 失败必须回滚注册：否则占位运行永不结束，持续占用会话活跃配额。
    let supervised = match if tty {
        spawn_tty_command(context, run.clone())
    } else {
        spawn_pipe_command(context, run.clone())
    } {
        Ok(supervised) => supervised,
        Err(error) => {
            reap(&run.run_id, &run.workspace_id);
            return Err(error);
        }
    };
    tokio::spawn(supervised);
    Ok(run)
}

/// 在等待窗口内观察运行；窗口耗尽仍运行时返回 `Running` 快照。
pub(super) async fn wait_for(run: &Arc<BackgroundRun>, window: Duration) -> RunSnapshot {
    let mut done = run.done_rx.clone();
    if !*done.borrow() {
        let changed = tokio::time::timeout(window, async {
            loop {
                match done.changed().await {
                    // 发送端随监督器结束而 drop：同样视为完成。
                    Ok(()) if *done.borrow() => return,
                    Ok(()) => {}
                    Err(_) => return,
                }
            }
        })
        .await;
        if changed.is_err() {
            return snapshot_running(run);
        }
    }
    snapshot_finished(run)
}

fn snapshot_running(run: &Arc<BackgroundRun>) -> RunSnapshot {
    let state = run.state.lock();
    // 截断判定用原始缓冲长度：文本快照经 trim 后长度回推会误报截断。
    let output_truncated = state.total_bytes > state.stdout.len() + state.stderr.len();
    RunSnapshot::Running {
        run_id: run.run_id.clone(),
        elapsed_ms: run.started_at.elapsed().as_millis(),
        stdout: String::from_utf8_lossy(&state.stdout)
            .trim_end()
            .to_string(),
        stderr: String::from_utf8_lossy(&state.stderr)
            .trim_end()
            .to_string(),
        output_truncated,
    }
}

fn snapshot_finished(run: &Arc<BackgroundRun>) -> RunSnapshot {
    let mut state = run.state.lock();
    // 监督器置位 finished 前已完成审计写入；此处仅做数据搬移。
    debug_assert!(state.finished, "完成信号先于 finished 标记：监督器顺序违约");
    let finished = FinishedRun {
        exit_code: state.exit_code,
        timed_out: state.timed_out,
        cancelled: state.cancelled,
        duration_ms: state.duration_ms,
        stdout: String::from_utf8_lossy(&state.stdout)
            .trim_end()
            .to_string(),
        stderr: String::from_utf8_lossy(&state.stderr)
            .trim_end()
            .to_string(),
        output_truncated: state.output_truncated,
        history: state.history.take().unwrap_or_default(),
        audit_error: state.audit_error.clone(),
    };
    RunSnapshot::Finished(Box::new(finished))
}

// ── 注册表 ────────────────────────────────────────────────────────────

fn registry() -> &'static Mutex<HashMap<String, Arc<BackgroundRun>>> {
    static REGISTRY: std::sync::OnceLock<Mutex<HashMap<String, Arc<BackgroundRun>>>> =
        std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register(run: &Arc<BackgroundRun>) -> Result<(), String> {
    let mut registry = registry().lock();
    let session_runs: Vec<Arc<BackgroundRun>> = registry
        .values()
        .filter(|item| item.workspace_id == run.workspace_id)
        .cloned()
        .collect();
    let active = session_runs
        .iter()
        .filter(|item| !is_finished(item))
        .count();
    if active >= MAX_ACTIVE_RUNS_PER_SESSION {
        return Err(format!(
            "错误：当前会话已有 {active} 条并行后台命令（上限 {MAX_ACTIVE_RUNS_PER_SESSION}），请先用 wait_run_id 等待或收割既有运行"
        ));
    }
    if session_runs.len() >= MAX_TRACKED_RUNS_PER_SESSION {
        // 淘汰最旧的已完成条目；全是活跃条目时不可能走到（受 active 上限约束）。
        if let Some(evict) = session_runs
            .iter()
            .filter(|item| is_finished(item))
            .min_by_key(|item| item.started_at)
            .map(|item| item.run_id.clone())
        {
            registry.remove(&evict);
        }
    }
    registry.insert(run.run_id.clone(), run.clone());
    Ok(())
}

fn is_finished(run: &Arc<BackgroundRun>) -> bool {
    *run.done_rx.borrow()
}

/// 查找运行；只允许同一会话（workspace）收割，跨会话探测按不存在处理。
pub(super) fn lookup(run_id: &str, workspace_id: &str) -> Option<Arc<BackgroundRun>> {
    registry()
        .lock()
        .get(run_id)
        .filter(|run| run.workspace_id == workspace_id)
        .cloned()
}

/// 收割：从注册表移除（监督器已结束，移除只是丢掉查表入口）。
pub(super) fn reap(run_id: &str, workspace_id: &str) {
    registry()
        .lock()
        .retain(|_, run| !(run.run_id == run_id && run.workspace_id == workspace_id));
}

// ── 监督器 ────────────────────────────────────────────────────────────

enum SupervisedChild {
    Pipes(tokio::process::Child),
    Tty {
        child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
        pid: Option<u32>,
    },
}

struct ExitOutcome {
    exit_code: Option<i32>,
}

enum WaitOutcome {
    Exited(ExitOutcome),
    /// 等待子进程退出本身失败（罕见）：按失败收口，仍走终止/审计全流程。
    WaitFailed(String),
    TimedOut,
    Cancelled,
}

/// 输出归属：tty 合并流记 stdout；管道模式两路各自归属。
type PumpTarget = fn(&mut RunState, &[u8]) -> usize;

fn append_bounded(buffer: &mut Vec<u8>, chunk: &[u8]) -> usize {
    let remaining = MAX_OUTPUT_BYTES.saturating_sub(buffer.len());
    if remaining > 0 {
        buffer.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    chunk.len()
}

fn pump_target_stdout(state: &mut RunState, chunk: &[u8]) -> usize {
    append_bounded(&mut state.stdout, chunk)
}

fn pump_target_stderr(state: &mut RunState, chunk: &[u8]) -> usize {
    append_bounded(&mut state.stderr, chunk)
}

/// 管道模式：zsh -lc，stdout/stderr 分流收集，独立进程组。
fn spawn_pipe_command(
    context: SupervisorContext,
    run: Arc<BackgroundRun>,
) -> Result<SupervisedFuture, String> {
    let mut cmd = Command::new("/bin/zsh");
    cmd.arg("-lc")
        .arg(&context.command)
        .current_dir(&context.run_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(temp_dir) = context.temp_dir.as_deref() {
        cmd.env("TMPDIR", temp_dir);
    }
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd
        .spawn()
        .map_err(|error| format!("错误：执行 zsh 命令失败：{error}"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let stdout_task = tokio::spawn(pump_async(
        stdout,
        run.clone(),
        pump_target_stdout as PumpTarget,
    ));
    let stderr_task = tokio::spawn(pump_async(
        stderr,
        run.clone(),
        pump_target_stderr as PumpTarget,
    ));
    Ok(Box::pin(supervise(
        SupervisedChild::Pipes(child),
        stdout_task,
        stderr_task,
        context,
        run,
    )))
}

/// tty 模式：PTY 会话（子进程 setsid 成为会话组长），stdout/stderr 合并
/// 单流；适合交互式或需要终端行为的命令。
fn spawn_tty_command(
    context: SupervisorContext,
    run: Arc<BackgroundRun>,
) -> Result<SupervisedFuture, String> {
    let pty_system = portable_pty::native_pty_system();
    let pair = pty_system
        .openpty(portable_pty::PtySize {
            rows: 24,
            cols: 200,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("错误：打开 PTY 失败：{error}"))?;
    let mut command = portable_pty::CommandBuilder::new("/bin/zsh");
    command.arg("-lc");
    command.arg(&context.command);
    command.cwd(&context.run_dir);
    if let Some(temp_dir) = context.temp_dir.as_deref() {
        command.env("TMPDIR", temp_dir.to_string_lossy().as_ref());
    }
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("错误：PTY 启动 zsh 失败：{error}"))?;
    // slave 句柄尽早丢弃：子进程退出后 master reader 才能读到 EOF。
    drop(pair.slave);
    let reader = match pair.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(error) => {
            // 失败路径不得遗留已启动的子进程。
            let _ = child.kill();
            return Err(format!("错误：克隆 PTY reader 失败：{error}"));
        }
    };
    let pid = child.process_id();
    let child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>> = Arc::new(Mutex::new(child));

    let reader_state = run.clone();
    let reader_task = tokio::task::spawn_blocking(move || {
        pump_blocking(reader, reader_state, pump_target_stdout as PumpTarget)
    });
    let no_stderr = tokio::spawn(std::future::ready(()));

    Ok(Box::pin(supervise(
        SupervisedChild::Tty { child, pid },
        reader_task,
        no_stderr,
        context,
        run,
    )))
}

async fn pump_async(
    reader: Option<impl tokio::io::AsyncRead + Unpin>,
    run: Arc<BackgroundRun>,
    target: PumpTarget,
) {
    let Some(mut reader) = reader else { return };
    // 32KB 缓冲（项目 PTY/流式读取约定下限），降低 read 与锁的频次。
    let mut chunk = [0u8; 32 * 1024];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut state = run.state.lock();
                state.total_bytes += target(&mut state, &chunk[..n]);
            }
        }
    }
}

fn pump_blocking(mut reader: impl Read, run: Arc<BackgroundRun>, target: PumpTarget) {
    let mut chunk = [0u8; 32 * 1024];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut state = run.state.lock();
                state.total_bytes += target(&mut state, &chunk[..n]);
            }
        }
    }
}

type SupervisedFuture = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

async fn supervise(
    mut child: SupervisedChild,
    mut pump_stdout: tokio::task::JoinHandle<()>,
    mut pump_stderr: tokio::task::JoinHandle<()>,
    context: SupervisorContext,
    run: Arc<BackgroundRun>,
) {
    let timeout_secs = context.exec_timeout_secs.max(1);
    let started = Instant::now();

    let outcome = {
        let deadline = tokio::time::sleep(Duration::from_secs(timeout_secs));
        tokio::pin!(deadline);
        let cancellation = wait_for_cancellation(context.cancel_rx.clone());
        tokio::pin!(cancellation);
        tokio::select! {
            biased;
            status = wait_child(&mut child) => match status {
                Ok(outcome) => WaitOutcome::Exited(outcome),
                Err(error) => WaitOutcome::WaitFailed(format!("错误：等待 zsh 退出失败：{error}")),
            },
            _ = &mut cancellation => WaitOutcome::Cancelled,
            _ = &mut deadline => WaitOutcome::TimedOut,
        }
    };

    let mut wait_error = None;
    let (exit_code, timed_out, cancelled) = match outcome {
        WaitOutcome::Exited(outcome) => (outcome.exit_code, false, false),
        WaitOutcome::WaitFailed(error) => {
            // 等待失败后子进程状态未知：按进程组终止并回收。PTY 模式没有
            // kill_on_drop 兜底，遗留进程既无人等待也不再受硬超时约束。
            kill_child(&mut child);
            let outcome = wait_child_reap(&mut child).await;
            wait_error = Some(error);
            (outcome.exit_code, false, false)
        }
        WaitOutcome::TimedOut => {
            kill_child(&mut child);
            let outcome = wait_child_reap(&mut child).await;
            (outcome.exit_code, true, false)
        }
        WaitOutcome::Cancelled => {
            kill_child(&mut child);
            let outcome = wait_child_reap(&mut child).await;
            (outcome.exit_code, false, true)
        }
    };
    // 子进程退出后给输出泵有限宽限：派生的孙进程继承管道/PTY 时读不到 EOF，
    // 无限等待会让 run 永不结束（占住活跃配额、审计与台账不落盘）。
    let grace = Duration::from_secs(2);
    if tokio::time::timeout(grace, &mut pump_stdout).await.is_err() {
        pump_stdout.abort();
    }
    if tokio::time::timeout(grace, &mut pump_stderr).await.is_err() {
        pump_stderr.abort();
    }
    let duration_ms = started.elapsed().as_millis();

    // 审计与命令台账先于完成信号写入：收割方看到的终态自带历史。
    let (stdout_text, stderr_text, output_truncated) = {
        let state = run.state.lock();
        (
            String::from_utf8_lossy(&state.stdout)
                .trim_end()
                .to_string(),
            String::from_utf8_lossy(&state.stderr)
                .trim_end()
                .to_string(),
            state.total_bytes > state.stdout.len() + state.stderr.len(),
        )
    };
    let entry = LocalZshAuditEntry {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: context.workspace_id.clone(),
        executed_at: chrono::Utc::now().to_rfc3339(),
        command: context.command.clone(),
        working_directory: Some(context.run_dir.display().to_string()),
        review: Some(context.review.clone()),
        exit_code,
        timed_out,
        cancelled,
        duration_ms,
        stdout: stdout_text,
        stderr: stderr_text,
        output_truncated,
        error: wait_error,
    };
    let history_note = history_note(
        &context,
        exit_code,
        timed_out,
        cancelled,
        &entry.stdout,
        &entry.stderr,
    );
    command_history::record(
        &context.workspace_id,
        "local_zsh",
        "本地 zsh",
        &context.command,
        CommandHistoryStatus::Executed,
        &history_note,
    );
    let audit_dir = context.audit_dir.clone();
    let audit_entry = entry;
    let session_id = context.workspace_id.clone();
    let audit_result = tokio::task::spawn_blocking(move || {
        super::append_audit_entry(&audit_dir, audit_entry, &session_id)
    })
    .await;

    let (history, audit_error) = match audit_result {
        Ok(Ok(history)) => (Some(history), None),
        Ok(Err(error)) => (None, Some(error)),
        Err(join_error) => (None, Some(format!("{join_error}"))),
    };
    {
        let mut state = run.state.lock();
        state.exit_code = exit_code;
        state.timed_out = timed_out;
        state.cancelled = cancelled;
        state.duration_ms = duration_ms;
        state.output_truncated = output_truncated;
        state.history = history;
        state.audit_error = audit_error;
        state.finished = true;
    }
    // done_rx 的订阅者（含注册表的 is_finished 探测）从此可见终态。
    let _ = run.done_tx.send(true);
}

async fn wait_child(child: &mut SupervisedChild) -> std::io::Result<ExitOutcome> {
    match child {
        SupervisedChild::Pipes(child) => child.wait().await.map(|status| ExitOutcome {
            exit_code: status.code(),
        }),
        SupervisedChild::Tty { child, .. } => {
            let waiter = child.clone();
            tokio::task::spawn_blocking(move || {
                waiter.lock().wait().map(|status| ExitOutcome {
                    exit_code: i32::try_from(status.exit_code()).ok(),
                })
            })
            .await
            .map_err(std::io::Error::other)?
        }
    }
}

async fn wait_child_reap(child: &mut SupervisedChild) -> ExitOutcome {
    wait_child(child)
        .await
        .unwrap_or(ExitOutcome { exit_code: None })
}

fn kill_child(child: &mut SupervisedChild) {
    match child {
        SupervisedChild::Pipes(child) => {
            // 独立进程组（spawn 时 process_group(0)）：杀整组含派生孙进程。
            if let Some(pid) = child.id() {
                #[cfg(unix)]
                unsafe {
                    libc::killpg(pid as libc::pid_t, libc::SIGKILL);
                }
            }
            let _ = child.start_kill();
        }
        SupervisedChild::Tty { child, pid } => {
            // PTY 子进程 setsid 成为会话组长，pid 即进程组 id。
            #[cfg(unix)]
            if let Some(pid) = *pid {
                unsafe {
                    libc::killpg(pid as libc::pid_t, libc::SIGKILL);
                }
            }
            // 等待侧可能正持锁阻塞在 wait() 上（spawn_blocking 内）；异步线程
            // 不能同步抢锁，killpg 已发信号，拿不到锁时由等待侧随退出收口。
            if let Some(mut child) = child.try_lock() {
                let _ = child.kill();
            }
        }
    }
}

async fn wait_for_cancellation(mut cancel_rx: Option<watch::Receiver<bool>>) {
    let Some(cancel_rx) = cancel_rx.as_mut() else {
        std::future::pending::<()>().await;
        return;
    };
    if *cancel_rx.borrow() {
        return;
    }
    loop {
        match cancel_rx.changed().await {
            Ok(()) if *cancel_rx.borrow() => return,
            Ok(()) => {}
            Err(_) => return,
        }
    }
}

fn history_note(
    context: &SupervisorContext,
    exit_code: Option<i32>,
    timed_out: bool,
    cancelled: bool,
    stdout: &str,
    stderr: &str,
) -> String {
    let note = if timed_out {
        format!("超时终止（{}s）", context.exec_timeout_secs.max(1))
    } else if cancelled {
        "已取消".to_string()
    } else {
        let exit = exit_code
            .map(|code| format!("exit={code}"))
            .unwrap_or_else(|| "exit=unknown".to_string());
        let excerpt = if !stdout.trim().is_empty() {
            stdout
        } else {
            stderr
        };
        format!("{exit}；输出：{excerpt}")
    };
    if context.history_with_cwd {
        format!("工作目录：{}；{note}", context.run_dir.display())
    } else {
        note
    }
}

#[cfg(test)]
#[path = "sessions_tests.rs"]
mod tests;

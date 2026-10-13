//! JSON 行协议与进程树生命周期；动作请求超时由会话层终止服务，预览请求独立观察且保持传输完整。
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{oneshot, watch, Mutex as AsyncMutex};

use super::{BrowserAgentEventCallback, BrowserStatus};

pub(super) type EventSink = Arc<dyn Fn(&str, Value) + Send + Sync>;

struct AgentBinding {
    run_id: String,
    tool_call_id: String,
    callback: BrowserAgentEventCallback,
}

pub(super) struct BrowserProcess {
    pub(super) session_id: String,
    stdin: AsyncMutex<ChildStdin>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    status: Mutex<BrowserStatus>,
    next_id: AtomicU64,
    alive: AtomicBool,
    killed: AtomicBool,
    pid: u32,
    exited: watch::Receiver<Option<Result<(), String>>>,
    termination_done: watch::Sender<bool>,
    events: EventSink,
    agent: Mutex<Option<AgentBinding>>,
    stream_id: String,
    last_frame_sequence: AtomicU64,
    preview_attached: AtomicBool,
    diagnostics: Mutex<VecDeque<String>>,
    termination_error: Mutex<Option<String>>,
}

impl BrowserProcess {
    pub(super) fn status(&self) -> BrowserStatus {
        self.status.lock().clone()
    }
    pub(super) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    pub(super) fn has_exited(&self) -> bool {
        matches!(&*self.exited.borrow(), Some(Ok(()))) && self.termination_error.lock().is_none()
    }

    pub(super) fn set_agent(
        &self,
        run_id: String,
        tool_call_id: String,
        callback: BrowserAgentEventCallback,
    ) {
        *self.agent.lock() = Some(AgentBinding {
            run_id,
            tool_call_id,
            callback,
        });
        self.set_state("automating", Some("浏览器探索 Agent 正在执行".into()));
    }

    pub(super) fn finish_agent(&self) {
        self.agent.lock().take();
        if self.is_alive() {
            self.set_state("ready", None);
        }
    }

    fn set_state(&self, state: &str, message: Option<String>) {
        let status = {
            let mut status = self.status.lock();
            status.state = state.into();
            status.message = message;
            status.clone()
        };
        (self.events)("browser-status", json!(status));
    }

    pub(super) async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        if !self.is_alive() {
            return Err("浏览器 Python 服务已经退出，请重新启动".into());
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id, tx);
        let _pending = PendingRequest {
            pending: &self.pending,
            id,
        };
        let line = serde_json::to_vec(&json!({"id": id, "method": method, "params": params}))
            .map_err(|error| format!("序列化浏览器请求失败：{error}"))?;
        {
            let mut stdin = self.stdin.lock().await;
            stdin
                .write_all(&line)
                .await
                .map_err(|e| format!("写入浏览器服务失败：{e}"))?;
            stdin
                .write_all(b"\n")
                .await
                .map_err(|e| format!("写入浏览器服务失败：{e}"))?;
            stdin
                .flush()
                .await
                .map_err(|e| format!("刷新浏览器服务请求失败：{e}"))?;
        }
        rx.await
            .map_err(|_| "浏览器服务响应通道已关闭".to_string())?
    }

    /// Drop 无需异步运行时：Unix 同步发信号，Windows 独立线程确认 taskkill 结果。
    pub(super) fn abort(self: &Arc<Self>, reason: &str) {
        #[cfg(unix)]
        self.abort_with(reason, terminate_process_group);
        #[cfg(windows)]
        self.abort_in_background(reason, termination::taskkill);
    }

    #[cfg(any(unix, test))]
    fn abort_with(&self, reason: &str, terminate: impl FnOnce(u32) -> Result<(), String>) {
        if self.begin_abort() {
            self.finish_abort(reason, terminate(self.pid));
        }
    }

    fn begin_abort(&self) -> bool {
        self.alive.store(false, Ordering::Release);
        if self.killed.swap(true, Ordering::AcqRel) {
            return false;
        }
        self.agent.lock().take();
        true
    }

    #[cfg(any(windows, test))]
    fn abort_in_background(
        self: &Arc<Self>,
        reason: &str,
        terminate: impl FnOnce(u32) -> Result<(), String> + Send + 'static,
    ) {
        if !self.begin_abort() {
            return;
        }
        self.set_state("stopping", Some(reason.into()));
        let process = self.clone();
        let owned_reason = reason.to_string();
        let worker = std::thread::Builder::new()
            .name("browser-terminate".into())
            .spawn(move || process.finish_abort(&owned_reason, terminate(process.pid)));
        if let Err(error) = worker {
            self.finish_abort(reason, Err(format!("启动终止线程失败：{error}")));
        }
    }

    fn finish_abort(&self, reason: &str, termination: Result<(), String>) {
        let (state, message) = match termination {
            Ok(()) => ("closed", reason.to_string()),
            Err(error) => {
                let message = format!("浏览器停止未确认：{reason}；终止进程树失败：{error}");
                *self.termination_error.lock() = Some(message.clone());
                ("error", message)
            }
        };
        for (_, tx) in self.pending.lock().drain() {
            let _ = tx.send(Err(message.clone()));
        }
        self.set_state(state, Some(message));
        self.termination_done.send_replace(true);
    }

    pub(super) async fn terminate(self: &Arc<Self>, reason: &str) -> Result<(), String> {
        #[cfg(unix)]
        let retry = |pid| std::future::ready(terminate_process_group(pid));
        #[cfg(windows)]
        let retry = |pid| async move {
            tokio::task::spawn_blocking(move || termination::taskkill(pid))
                .await
                .map_err(|error| format!("浏览器终止任务失败：{error}"))?
        };
        self.terminate_with(reason, retry).await
    }

    async fn terminate_with<F: std::future::Future<Output = Result<(), String>>>(
        self: &Arc<Self>,
        reason: &str,
        retry: impl FnOnce(u32) -> F,
    ) -> Result<(), String> {
        self.abort(reason);
        let mut exited = self.exited.clone();
        let mut result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Some(result) = exited.borrow_and_update().clone() {
                    return result;
                }
                exited
                    .changed()
                    .await
                    .map_err(|_| "浏览器进程回收通道已关闭".to_string())?;
            }
        })
        .await
        .unwrap_or_else(|_| Err("终止浏览器进程后，5 秒内仍未确认退出".to_string()));
        // 子进程退出可能与首次 killpg 交错；确认回收后重试一次，不能仅凭父进程
        // 已退出就忽略 EPERM，因为同组 Chromium 子进程仍可能存活。
        if result.is_ok() && self.termination_error.lock().is_some() {
            result = retry(self.pid)
                .await
                .map_err(|error| format!("浏览器停止未确认：终止进程树失败：{error}"));
        }
        if let Err(error) = &result {
            *self.termination_error.lock() = Some(error.clone());
            self.set_state("error", Some(error.clone()));
        } else {
            self.termination_error.lock().take();
            self.set_state("closed", Some(reason.to_string()));
        }
        result
    }

    fn handle_message(&self, value: Value) {
        if !self.is_alive() {
            return;
        }
        if let Some(id) = value.get("id").and_then(Value::as_u64) {
            if let Some(tx) = self.pending.lock().remove(&id) {
                let result = if value.get("ok").and_then(Value::as_bool) == Some(true) {
                    Ok(value.get("result").cloned().unwrap_or(Value::Null))
                } else {
                    // 错误分类只由 Rust 生命周期边界决定，不信任 sidecar 文本前缀。
                    Err(format!(
                        "浏览器服务错误：{}",
                        value
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("未知错误")
                    ))
                };
                let _ = tx.send(result);
            }
            return;
        }
        if value.get("sessionId").and_then(Value::as_str) != Some(&self.session_id) {
            return;
        }
        match value.get("event").and_then(Value::as_str) {
            Some("agent") => {
                let callback = self.agent.lock().as_ref().and_then(|binding| {
                    (value.get("runId").and_then(Value::as_str) == Some(binding.run_id.as_str()))
                        .then(|| binding.callback.clone())
                });
                if let Some(callback) = callback {
                    callback(value);
                }
            }
            Some("status") => {
                if let Some(mut status) = status_from_value(&value) {
                    if self.agent.lock().is_some() && status.state != "closed" {
                        status.state = "automating".into();
                    }
                    *self.status.lock() = status.clone();
                    (self.events)("browser-status", json!(status));
                }
            }
            Some("frame") => self.handle_frame(value),
            Some("preview_error") => self.handle_preview_error(value),
            Some("log") => (self.events)("browser-log", value),
            _ => {}
        }
    }

    fn exit_message(&self) -> String {
        let details = self
            .diagnostics
            .lock()
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        if details.is_empty() {
            "浏览器 Python 服务已退出；请检查 uv 或 JKC_BROWSER_PYTHON 环境".into()
        } else {
            format!("浏览器 Python 服务已退出：\n{details}")
        }
    }
}

struct PendingRequest<'a> {
    pending: &'a Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    id: u64,
}
impl Drop for PendingRequest<'_> {
    fn drop(&mut self) {
        self.pending.lock().remove(&self.id);
    }
}

pub(super) fn spawn(
    mut command: Command,
    session_id: String,
    events: EventSink,
) -> Result<Arc<BrowserProcess>, String> {
    command
        .env("JKC_BROWSER_SESSION_ID", &session_id)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|error| format!(
        "启动 browser-use Python 服务失败：{error}。请安装 uv，或设置 JKC_BROWSER_PYTHON 指向已安装 browser-use==0.13.11 的 Python。"
    ))?;
    let pid = child
        .id()
        .ok_or_else(|| "浏览器服务没有进程标识".to_string())?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "浏览器服务 stdin 不可用".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "浏览器服务 stdout 不可用".to_string())?;
    let stderr = child.stderr.take();
    let (exit_tx, exited) = watch::channel(None);
    let (termination_done, _) = watch::channel(false);
    let process = Arc::new(BrowserProcess {
        status: Mutex::new(BrowserStatus {
            session_id: session_id.clone(),
            state: "starting".into(),
            url: None,
            message: None,
        }),
        session_id,
        stdin: AsyncMutex::new(stdin),
        pending: Mutex::new(HashMap::new()),
        next_id: AtomicU64::new(1),
        alive: AtomicBool::new(true),
        killed: AtomicBool::new(false),
        pid,
        exited,
        termination_done,
        events,
        agent: Mutex::new(None),
        stream_id: uuid::Uuid::new_v4().to_string(),
        last_frame_sequence: AtomicU64::new(0),
        preview_attached: AtomicBool::new(false),
        diagnostics: Mutex::new(VecDeque::new()),
        termination_error: Mutex::new(None),
    });
    process.set_state("starting", Some("正在启动 browser-use 服务与浏览器".into()));
    let reader = process.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            match serde_json::from_str::<Value>(&line) {
                Ok(value) => reader.handle_message(value),
                Err(_) if reader.is_alive() => (reader.events)(
                    "browser-log",
                    json!({"sessionId": reader.session_id, "message": line}),
                ),
                Err(_) => {}
            }
        }
        // stderr 的致命尾部（Python traceback 等）常滞后于 stdout EOF；
        // 先给诊断环 drain 宽限再快照退出原因（对齐 rag 的做法）。
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        reader.abort(&reader.exit_message());
    });
    let monitor = process.clone();
    tokio::spawn(async move {
        let result = child
            .wait()
            .await
            .map(|_| ())
            .map_err(|error| format!("浏览器进程回收失败：{error}"));
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        monitor.abort(&monitor.exit_message());
        // 另一个线程可能已进入 abort，但还没记录 killpg 结果。退出通知必须晚于
        // 该次终止尝试完成，否则 terminate 会错过尚未发布的真实权限错误。
        let mut termination_done = monitor.termination_done.subscribe();
        while !*termination_done.borrow_and_update() {
            if termination_done.changed().await.is_err() {
                return;
            }
        }
        exit_tx.send_replace(Some(result));
    });
    if let Some(stderr) = stderr {
        let reader = process.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                // 诊断环不受 alive 门禁：进程退出后的 stderr 尾部往往正是崩溃
                // 真因，exit_message 需要它们；日志转发仍只在存活时进行。
                {
                    let mut diagnostics = reader.diagnostics.lock();
                    if diagnostics.len() >= 8 {
                        diagnostics.pop_front();
                    }
                    diagnostics.push_back(line.chars().take(500).collect());
                }
                if reader.is_alive() {
                    (reader.events)(
                        "browser-log",
                        json!({"sessionId": reader.session_id, "message": line}),
                    );
                }
            }
        });
    }
    Ok(process)
}

#[cfg(unix)]
fn terminate_process_group(pid: u32) -> Result<(), String> {
    // 进程由 process_group(0) 单独建组；负 pid 覆盖 uv、Python 与 Chromium 子树。
    let result = unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error.to_string())
    }
}

#[cfg(any(windows, test))]
mod termination;

mod preview;

pub(super) fn status_from_value(value: &Value) -> Option<BrowserStatus> {
    let status = value.get("status").unwrap_or(value);
    serde_json::from_value(status.clone()).ok()
}

#[cfg(all(test, unix))]
mod tests;

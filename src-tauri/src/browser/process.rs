use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::Mutex as ParkingMutex;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};
use tokio::time::{timeout, Duration};

use crate::platform::get_login_shell_path;

use super::paths::{resolve_driver_path, resolve_node_modules_hint, resolve_node_path};
use super::BrowserStatus;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserFrameEvent {
    session_id: String,
    data: String,
    width: u32,
    height: u32,
}

pub(super) struct BrowserProcess {
    pub(super) session_id: String,
    #[allow(dead_code)]
    project_path: String,
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    pub(super) status: ParkingMutex<BrowserStatus>,
    next_id: AtomicU64,
}

impl BrowserProcess {
    pub(super) fn status(&self) -> BrowserStatus {
        self.status.lock().clone()
    }

    pub(super) async fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        request_timeout: Duration,
    ) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let payload = json!({
            "id": id,
            "method": method,
            "params": params
        });
        let line = serde_json::to_string(&payload).map_err(|error| error.to_string())?;
        let write_result = async {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(line.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await
        }
        .await;

        if let Err(error) = write_result {
            self.pending.lock().await.remove(&id);
            return Err(format!("写入 CloakBrowser sidecar 请求失败：{error}"));
        }

        match timeout(request_timeout, rx).await {
            Ok(result) => result.map_err(|_| "CloakBrowser sidecar 响应通道已关闭".to_string())?,
            Err(_) => {
                self.pending.lock().await.remove(&id);
                let status = self.status();
                let status_detail = status
                    .message
                    .as_deref()
                    .filter(|message| !message.trim().is_empty())
                    .unwrap_or(&status.state);
                Err(format!(
                    "CloakBrowser 工具调用超时（method={method}，等待 {} 秒）。当前浏览器状态：{status_detail}",
                    request_timeout.as_secs()
                ))
            }
        }
    }

    pub(super) async fn kill(&self) -> Result<(), String> {
        let mut child = self.child.lock().await;
        match child.kill().await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => Ok(()),
            Err(error) => Err(format!("关闭 CloakBrowser sidecar 失败：{error}")),
        }
    }
}

pub(super) fn empty_to_null(value: &str) -> Value {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Value::Null
    } else {
        Value::String(trimmed.to_string())
    }
}

pub(super) fn browser_command_timeout(params: &Value) -> Duration {
    let requested_ms = params
        .get("timeout")
        .and_then(Value::as_u64)
        .unwrap_or(30_000);
    let padded_ms = requested_ms.saturating_add(15_000).clamp(1_000, 180_000);
    Duration::from_millis(padded_ms)
}

pub(super) async fn spawn_sidecar(
    app: &AppHandle,
    session_id: &str,
    project_path: &str,
) -> Result<Arc<BrowserProcess>, String> {
    let driver_path = resolve_driver_path(app)?;
    let node_path = resolve_node_path(app);
    let node_modules_hint = resolve_node_modules_hint(app);
    let shell_path = get_login_shell_path();

    let mut command = Command::new(&node_path);
    command
        .arg(&driver_path)
        .env("JKC_BROWSER_SESSION_ID", session_id)
        .env("JKC_BROWSER_NODE_MODULES", node_modules_hint)
        .env("PATH", shell_path)
        .env("NODE_NO_WARNINGS", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    if let Some(cwd) = driver_path.parent() {
        command.current_dir(cwd);
    }

    let mut child = command.spawn().map_err(|error| {
        format!(
            "启动 CloakBrowser Node sidecar 失败：{error}。已尝试执行：{} {}",
            node_path.display(),
            driver_path.display()
        )
    })?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "CloakBrowser sidecar stdin 不可用".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "CloakBrowser sidecar stdout 不可用".to_string())?;
    let stderr = child.stderr.take();

    let process = Arc::new(BrowserProcess {
        session_id: session_id.to_string(),
        project_path: project_path.to_string(),
        stdin: Mutex::new(stdin),
        child: Mutex::new(child),
        pending: Mutex::new(HashMap::new()),
        status: ParkingMutex::new(BrowserStatus {
            session_id: session_id.to_string(),
            state: "starting".to_string(),
            url: None,
            message: Some("正在启动 CloakBrowser sidecar".to_string()),
            minimized: false,
            has_headed_window: false,
        }),
        next_id: AtomicU64::new(1),
    });

    spawn_stdout_reader(app.clone(), Arc::clone(&process), stdout);
    if let Some(stderr) = stderr {
        spawn_stderr_reader(app.clone(), session_id.to_string(), stderr);
    }

    Ok(process)
}

fn spawn_stdout_reader(
    app: AppHandle,
    process: Arc<BrowserProcess>,
    stdout: tokio::process::ChildStdout,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                emit_log(&app, &process.session_id, format!("sidecar stdout: {line}"));
                continue;
            };
            if let Some(id) = value.get("id").and_then(Value::as_u64) {
                let result = if value.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                    Ok(value.get("result").cloned().unwrap_or(Value::Null))
                } else {
                    let error_msg = value
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("CloakBrowser sidecar 返回未知错误")
                        .to_string();
                    let error_type = value
                        .get("errorType")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let formatted = if error_type.is_empty() {
                        error_msg
                    } else {
                        format!("[{error_type}] {error_msg}")
                    };
                    Err(formatted)
                };
                if let Some(tx) = process.pending.lock().await.remove(&id) {
                    let _ = tx.send(result);
                }
                continue;
            }

            match value.get("event").and_then(Value::as_str) {
                Some("status") => {
                    let current_minimized = process.status().minimized;
                    let current_has_headed_window = process.status().has_headed_window;
                    let event_name = value.get("event").and_then(Value::as_str);
                    let opened_this_event = event_name == Some("opened")
                        || value
                            .get("opened")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                    let has_headed_window = current_has_headed_window || opened_this_event;
                    if let Some(status) =
                        status_from_value(&value, current_minimized, has_headed_window)
                    {
                        *process.status.lock() = status.clone();
                        let _ = app.emit("browser-status", status);
                    }
                }
                Some("page_closed") => {
                    let mut s = process.status();
                    s.state = "page_closed".to_string();
                    s.minimized = false;
                    s.has_headed_window = false;
                    s.message = Some("浏览器窗口已关闭，可在面板中重新打开".to_string());
                    *process.status.lock() = s.clone();
                    let _ = app.emit("browser-status", s);
                }
                Some("frame") => {
                    let data = value
                        .get("data")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let width = value.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
                    let height = value.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
                    let _ = app.emit(
                        "browser-frame",
                        BrowserFrameEvent {
                            session_id: process.session_id.clone(),
                            data,
                            width,
                            height,
                        },
                    );
                }
                Some("log") => {
                    if let Some(message) = value.get("message").and_then(Value::as_str) {
                        emit_log(&app, &process.session_id, message.to_string());
                    }
                }
                _ => {}
            }
        }
        let closed = BrowserStatus {
            session_id: process.session_id.clone(),
            state: "closed".to_string(),
            url: process.status().url,
            message: Some("CloakBrowser sidecar 已退出".to_string()),
            minimized: false,
            has_headed_window: false,
        };
        *process.status.lock() = closed.clone();
        let mut pending = process.pending.lock().await;
        for (_, tx) in pending.drain() {
            let _ = tx.send(Err("CloakBrowser sidecar 已退出".to_string()));
        }
        drop(pending);
        let _ = app.emit("browser-status", closed);
    });
}

fn spawn_stderr_reader(app: AppHandle, session_id: String, stderr: tokio::process::ChildStderr) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            emit_log(&app, &session_id, format!("sidecar stderr: {line}"));
        }
    });
}

fn emit_log(app: &AppHandle, session_id: &str, message: String) {
    let _ = app.emit(
        "browser-log",
        json!({
            "sessionId": session_id,
            "message": message
        }),
    );
}

pub(super) fn status_from_value(
    value: &Value,
    current_minimized: bool,
    current_has_headed_window: bool,
) -> Option<BrowserStatus> {
    let status_value = value.get("status").unwrap_or(value);
    let session_id = status_value
        .get("sessionId")
        .or_else(|| value.get("sessionId"))?
        .as_str()?
        .to_string();
    let state = status_value
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let minimized = status_value
        .get("minimized")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            if state == "minimized" {
                true
            } else if state == "closed" || state == "page_closed" {
                false
            } else {
                current_minimized
            }
        });
    let has_headed_window = status_value
        .get("hasHeadedWindow")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            if state == "closed" || state == "page_closed" {
                false
            } else {
                current_has_headed_window
            }
        });
    Some(BrowserStatus {
        session_id,
        state,
        url: status_value
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_string),
        message: status_value
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string),
        minimized,
        has_headed_window,
    })
}

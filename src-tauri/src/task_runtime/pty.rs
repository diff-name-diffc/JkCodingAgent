use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::Read;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

use super::session::{spawn_resume_session_watcher, spawn_status_session_watcher};
use crate::agent::DispatcherState;
use crate::platform::{claude_version_gte, get_agent_bin_checked, get_login_shell_env};
use crate::project::read_project_config;
use crate::shared::{ManagedPtySnapshot, TaskManager, TaskTerminationIntent};

const SESSION_WAIT_POLL: Duration = Duration::from_millis(50);
const SESSION_WAIT_MAX: Duration = Duration::from_millis(500);
const PTY_READ_BUFFER_SIZE: usize = 64 * 1024;
const PTY_EMIT_FLUSH_INTERVAL: Duration = Duration::from_millis(16);
const PTY_EMIT_MAX_BATCH_BYTES: usize = 64 * 1024;
const PTY_IDLE_OUTPUT_MAX_BYTES: usize = 256 * 1024;
/// 有界 channel 容量：满时 reader 线程阻塞，反压传播至 OS 内核 PTY 缓冲区，
/// 最终使写入进程（Claude/Codex）的 write() 系统调用阻塞，从源头限流。
const PTY_EMIT_CHANNEL_CAPACITY: usize = 1024;

fn has_task_session(app: &AppHandle, task_id: &str, is_codex: bool) -> bool {
    let tm = app.state::<TaskManager>();
    if is_codex {
        tm.codex_sessions.lock().contains_key(task_id)
    } else {
        tm.claude_sessions.lock().contains_key(task_id)
    }
}

/// 任务结束后，等待会话注册完成，最长等待 500ms。
async fn wait_for_session(app: &AppHandle, task_id: &str, is_codex: bool) {
    let deadline = Instant::now() + SESSION_WAIT_MAX;
    while Instant::now() < deadline {
        if has_task_session(app, task_id, is_codex) {
            return;
        }
        tokio::time::sleep(SESSION_WAIT_POLL).await;
    }
}

fn finalize_task_exit(
    app: &AppHandle,
    task_id: &str,
    exit_ok: bool,
    exit_code: Option<u32>,
    wait_error: Option<String>,
) {
    let termination_intent = {
        let tm = app.state::<TaskManager>();
        tm.take_task_termination_intent(task_id)
    };

    {
        let tm = app.state::<TaskManager>();
        tm.mark_finished(task_id);
        tm.remove_pty_handles(task_id);

        let codex_info = tm.codex_sessions.lock().remove(task_id);
        let codex_path = codex_info.map(|info| info.session_path);
        let claude_info = tm.claude_sessions.lock().remove(task_id);
        let claude_path = claude_info.as_ref().map(|info| info.session_path.clone());
        let mut claimed = tm.claimed_session_paths.lock();
        if let Some(path) = codex_path {
            claimed.remove(&path);
        }
        if let Some(path) = claude_path {
            claimed.remove(&path);
        }
    }

    if let Some(intent) = termination_intent {
        let status = termination_status_label(intent);
        let _ = app.emit(
            "task-status",
            serde_json::json!({ "task_id": task_id, "status": status }),
        );
        return;
    }

    let status = task_exit_status(exit_ok);
    let payload = if status == "failed" {
        let reason = task_exit_failure_reason(exit_code, wait_error.as_deref());
        serde_json::json!({ "task_id": task_id, "status": status, "failure_reason": reason })
    } else {
        serde_json::json!({ "task_id": task_id, "status": status })
    };
    let _ = app.emit("task-status", payload);
}

fn task_exit_status(exit_ok: bool) -> &'static str {
    if exit_ok {
        "done"
    } else {
        "failed"
    }
}

fn task_exit_failure_reason(exit_code: Option<u32>, wait_error: Option<&str>) -> String {
    if let Some(error) = wait_error {
        return format!("读取 Agent 进程退出状态失败：{error}");
    }

    match exit_code {
        Some(code) => format!("Agent 进程以非 0 状态退出，退出码：{code}"),
        None => "Agent 进程以非 0 状态退出，但未返回退出码".to_string(),
    }
}

fn request_task_termination(
    task_manager: &TaskManager,
    task_id: &str,
    intent: TaskTerminationIntent,
) -> Result<(), String> {
    task_manager.set_task_termination_intent(task_id, intent);
    task_manager.kill_child(task_id)
}

fn termination_status_label(intent: TaskTerminationIntent) -> &'static str {
    match intent {
        TaskTerminationIntent::Stopped => "stopped",
    }
}

// ── 共享 PTY 辅助函数 ────────────────────────────────────────────────────────

/// 设置 CommandBuilder 的标准环境变量。
fn setup_env(cmd: &mut CommandBuilder) {
    for (key, value) in get_login_shell_env() {
        cmd.env(key, value);
    }
    // 设置终端类型，使 Claude Code / Codex 输出正确的转义序列
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
}

#[derive(Clone, Copy)]
enum PtyEmitMode {
    Batched {
        flush_interval: Duration,
        max_batch_bytes: usize,
    },
}

fn emit_pty_event(app: &AppHandle, id: &str, event_name: &str, id_key: &str, data: String) {
    let seq = app.state::<TaskManager>().append_output(id, &data);
    let mut payload = serde_json::Map::new();
    payload.insert(
        id_key.to_string(),
        serde_json::Value::String(id.to_string()),
    );
    payload.insert("data".to_string(), serde_json::Value::String(data));
    if let Some(seq) = seq {
        payload.insert("seq".to_string(), serde_json::Value::from(seq));
    }
    let _ = app.emit(event_name, serde_json::Value::Object(payload));
}

fn flush_pty_batch(app: &AppHandle, id: &str, event_name: &str, id_key: &str, batch: &mut String) {
    if batch.is_empty() {
        return;
    }
    emit_pty_event(app, id, event_name, id_key, std::mem::take(batch));
}

fn trim_output_tail(text: &mut String, max_bytes: usize) {
    if text.len() <= max_bytes {
        return;
    }
    let target = text.len() - max_bytes;
    let boundary = text
        .char_indices()
        .find_map(|(idx, _)| (idx >= target).then_some(idx))
        .unwrap_or(text.len());
    if boundary > 0 {
        text.drain(..boundary);
    }
}

/// 在后台线程中读取 PTY 输出，向前端发送事件。
///
/// - `event_name`：Tauri 事件名（`"agent-output"` 或 `"shell-output"`）
/// - `id_key`：JSON payload 中的 ID 字段名（`"task_id"` 或 `"shell_id"`）
/// - `session_tx`：可选 channel，用于将原始文本转发给 session watcher
/// - `idle_callback`：可选回调，仅在 session watcher 显式判定“当前轮次完成”时触发，
///   参数为自上次触发以来累积的原始输出文本。可重复触发以支持多轮会话。
/// - `on_finish`：PTY 关闭后执行的可选清理回调
#[allow(clippy::too_many_arguments)]
fn spawn_pty_reader(
    app: AppHandle,
    id: String,
    event_name: &'static str,
    id_key: &'static str,
    emit_mode: PtyEmitMode,
    reader: Box<dyn Read + Send>,
    session_tx: Option<std::sync::mpsc::Sender<String>>,
    idle_callback: Option<Box<dyn Fn(String) + Send>>,
    on_finish: Option<Box<dyn FnOnce() + Send>>,
    force_idle_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) {
    tokio::task::spawn_blocking(move || {
        let mut reader = reader;
        let mut buf = [0u8; PTY_READ_BUFFER_SIZE];
        // 保存上次读取中不完整的 UTF-8 字节序列
        let mut leftover: Vec<u8> = Vec::new();
        let PtyEmitMode::Batched {
            flush_interval,
            max_batch_bytes,
        } = emit_mode;
        let (tx, rx) = std::sync::mpsc::sync_channel::<String>(PTY_EMIT_CHANNEL_CAPACITY);
        let emit_app = app.clone();
        let emit_id = id.clone();
        let worker = std::thread::spawn(move || {
            let mut batch = String::new();
            // 累积本轮输出，只有收到 session watcher 的显式 turn-complete 信号才回调。
            let mut accumulated_output = String::new();
            loop {
                match rx.recv_timeout(flush_interval) {
                    Ok(chunk) => {
                        batch.push_str(&chunk);
                        accumulated_output.push_str(&chunk);
                        trim_output_tail(&mut accumulated_output, PTY_IDLE_OUTPUT_MAX_BYTES);
                        if batch.len() >= max_batch_bytes {
                            flush_pty_batch(&emit_app, &emit_id, event_name, id_key, &mut batch);
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        flush_pty_batch(&emit_app, &emit_id, event_name, id_key, &mut batch);
                        let force = force_idle_flag
                            .as_ref()
                            .map(|f| f.load(std::sync::atomic::Ordering::Acquire))
                            .unwrap_or(false);

                        if force {
                            if let Some(flag) = force_idle_flag.as_ref() {
                                flag.store(false, std::sync::atomic::Ordering::Release);
                            }
                            if let Some(ref cb) = idle_callback {
                                if !accumulated_output.is_empty() {
                                    cb(std::mem::take(&mut accumulated_output));
                                }
                            }
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        flush_pty_batch(&emit_app, &emit_id, event_name, id_key, &mut batch);
                        break;
                    }
                }
            }
        });
        let (emit_tx, emit_worker) = (Some(tx), Some(worker));
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut combined = std::mem::take(&mut leftover);
                    combined.extend_from_slice(&buf[..n]);

                    let valid_len = match std::str::from_utf8(&combined) {
                        Ok(_) => combined.len(),
                        Err(e) => e.valid_up_to(),
                    };

                    if valid_len > 0 {
                        // SAFETY：已确认 valid_len 之前的字节为有效 UTF-8
                        let data = unsafe {
                            std::str::from_utf8_unchecked(&combined[..valid_len]).to_owned()
                        };
                        // session_tx 需要独立副本；data 本身留给 emit 路径 move，避免多余堆分配
                        if let Some(ref tx) = session_tx {
                            let _ = tx.send(data.clone());
                        }
                        if let Some(ref tx) = emit_tx {
                            match tx.try_send(data) {
                                Ok(()) => {}
                                Err(std::sync::mpsc::TrySendError::Full(data)) => {
                                    emit_pty_event(&app, &id, event_name, id_key, data);
                                }
                                Err(std::sync::mpsc::TrySendError::Disconnected(data)) => {
                                    emit_pty_event(&app, &id, event_name, id_key, data);
                                }
                            }
                        }
                    }

                    if valid_len < combined.len() {
                        leftover = combined[valid_len..].to_vec();
                    }
                }
            }
        }
        drop(emit_tx);
        if let Some(worker) = emit_worker {
            let _ = worker.join();
        }
        // session_tx 在此处被 drop，watcher 端的 Receiver 将收到 Disconnected 信号
        if let Some(f) = on_finish {
            f();
        }
    });
}

/// 在后台线程中轮询子进程退出状态，退出后调用 finalize_task_exit。
async fn spawn_exit_monitor(app: AppHandle, task_id: String, is_codex: bool) {
    loop {
        let exit_status = {
            let tm = app.state::<TaskManager>();
            match tm.try_wait_child(&task_id) {
                Ok(Some(status)) => Some(Ok(status)),
                Ok(None) => {
                    if tm.child_handles.lock().contains_key(&task_id) {
                        None
                    } else {
                        return;
                    }
                }
                Err(error) => Some(Err(error)),
            }
        };

        if let Some(result) = exit_status {
            match result {
                Ok(status) => {
                    let exit_ok = status.success();
                    let exit_code = if exit_ok {
                        None
                    } else {
                        Some(status.exit_code())
                    };
                    // 等待会话注册完成
                    wait_for_session(&app, &task_id, is_codex).await;
                    finalize_task_exit(&app, &task_id, exit_ok, exit_code, None);
                }
                Err(error) => {
                    finalize_task_exit(&app, &task_id, false, None, Some(error));
                }
            }
            return;
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// 为 Claude 命令构建 CommandBuilder，并根据 permission_mode 添加权限标志。
fn build_claude_cmd(agent_bin: &str, permission_mode: &str) -> CommandBuilder {
    let mut c = CommandBuilder::new(agent_bin);
    match permission_mode {
        "ask" => {
            c.arg("--permission-mode");
            c.arg("default");
        }
        "auto_edit" => {
            c.arg("--permission-mode");
            c.arg("acceptEdits");
        }
        "full_access" => {
            c.arg("--dangerously-skip-permissions");
        }
        _ => {}
    }
    c
}

// ── Tauri 命令 ───────────────────────────────────────────────────────────────

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn start_dispatcher_subprocess(
    app: AppHandle,
    task_manager: State<'_, TaskManager>,
    dispatcher_state: State<'_, DispatcherState>,
    task_id: String,
    project_path: String,
    prompt: String,
    agent: String,
    permission_mode: String,
    cols: Option<u16>,
    rows: Option<u16>,
    dispatcher_dispatch_id: String,
    dispatcher_session_id: String,
    dispatcher_description: String,
) -> Result<(), String> {
    if agent != "claude" && agent != "codex" {
        return Err(format!("未知 Agent：{agent}"));
    }

    // Clone values needed after spawn_blocking
    let project_path_for_post = project_path.clone();
    let agent_for_post = agent.clone();

    let (master, writer, child, reader, is_codex, pre_session_id) =
        tokio::task::spawn_blocking(move || {
            let pair = native_pty_system()
                .openpty(PtySize {
                    rows: rows.unwrap_or(50),
                    cols: cols.unwrap_or(220),
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| e.to_string())?;

            let config = read_project_config(project_path.clone())?;
            let final_prompt = if config.agent.prompt_prefix.is_empty() {
                prompt.clone()
            } else {
                format!("{}\n{}", config.agent.prompt_prefix, prompt)
            };

            let agent_bin = get_agent_bin_checked(&agent)?;
            let is_codex = agent == "codex";

            let saved_claude_version = config.agent.claude_version.clone();
            let use_explicit_session =
                !is_codex && claude_version_gte(&saved_claude_version, "2.1.87");

            let pre_session_id = if use_explicit_session {
                Some(uuid::Uuid::new_v4().to_string())
            } else {
                None
            };

            let mut cmd = if is_codex {
                let mut c = CommandBuilder::new(&agent_bin);
                c.arg("--");
                c.arg(&final_prompt);
                c
            } else {
                let mut c = build_claude_cmd(&agent_bin, &permission_mode);
                if let Some(ref sid) = pre_session_id {
                    c.arg("--session-id");
                    c.arg(sid);
                }
                c.arg("--");
                c.arg(&final_prompt);
                c
            };
            cmd.cwd(&project_path);
            setup_env(&mut cmd);

            let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
            drop(pair.slave);
            let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
            let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

            Ok::<_, String>((pair.master, writer, child, reader, is_codex, pre_session_id))
        })
        .await
        .map_err(|e| format!("spawn_blocking 失败: {e}"))??;

    task_manager.insert_pty_handles(&task_id, master, writer, child);

    let _ = app.emit(
        "task-status",
        serde_json::json!({ "task_id": task_id, "status": "running" }),
    );

    let (session_tx, session_rx) = std::sync::mpsc::channel::<String>();
    spawn_status_session_watcher(
        app.clone(),
        task_id.clone(),
        project_path_for_post,
        is_codex,
        session_rx,
        pre_session_id,
    );

    let force_idle = dispatcher_state.register_subprocess(
        &dispatcher_session_id,
        &task_id,
        &dispatcher_dispatch_id,
        &agent_for_post,
        &dispatcher_description,
    );

    let idle_cb: Option<Box<dyn Fn(String) + Send>> = {
        let app_idle = app.clone();
        let tid_idle = task_id.clone();
        Some(Box::new(move |output: String| {
            let _ = app_idle.emit(
                "dispatcher-subprocess-idle",
                serde_json::json!({
                    "task_id": tid_idle,
                    "output": output
                }),
            );
        }))
    };

    spawn_pty_reader(
        app.clone(),
        task_id.clone(),
        "agent-output",
        "task_id",
        PtyEmitMode::Batched {
            flush_interval: PTY_EMIT_FLUSH_INTERVAL,
            max_batch_bytes: PTY_EMIT_MAX_BATCH_BYTES,
        },
        reader,
        Some(session_tx),
        idle_cb,
        None,
        Some(force_idle),
    );
    tokio::spawn(spawn_exit_monitor(app, task_id, is_codex));

    Ok(())
}

#[tauri::command]
pub async fn stop_task(
    task_manager: State<'_, TaskManager>,
    task_id: String,
) -> Result<(), String> {
    request_task_termination(&task_manager, &task_id, TaskTerminationIntent::Stopped)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn resume_dispatcher_subprocess(
    app: AppHandle,
    task_manager: State<'_, TaskManager>,
    dispatcher_state: State<'_, DispatcherState>,
    task_id: String,
    project_path: String,
    agent: String,
    session_id: String,
    _prompt: String,
    permission_mode: String,
    cols: Option<u16>,
    rows: Option<u16>,
    dispatcher_dispatch_id: String,
    dispatcher_session_id: String,
    dispatcher_description: String,
) -> Result<(), String> {
    if agent != "claude" && agent != "codex" {
        return Err(format!("未知 Agent：{agent}"));
    }

    let agent_for_post = agent.clone();
    let project_path_for_post = project_path.clone();
    let session_id_for_post = session_id.clone();

    let (master, writer, child, reader) = tokio::task::spawn_blocking(move || {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: rows.unwrap_or(50),
                cols: cols.unwrap_or(220),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;

        let agent_bin = get_agent_bin_checked(&agent)?;
        let mut cmd = if agent == "codex" {
            let mut c = CommandBuilder::new(&agent_bin);
            c.arg("resume");
            c.arg(&session_id);
            c
        } else {
            let mut c = build_claude_cmd(&agent_bin, &permission_mode);
            c.arg("--resume");
            c.arg(&session_id);
            c
        };
        cmd.cwd(&project_path);
        setup_env(&mut cmd);

        let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

        Ok::<_, String>((pair.master, writer, child, reader))
    })
    .await
    .map_err(|e| format!("spawn_blocking 失败: {e}"))??;

    task_manager.insert_pty_handles(&task_id, master, writer, child);

    let _ = app.emit(
        "task-status",
        serde_json::json!({ "task_id": task_id, "status": "running" }),
    );

    let is_codex = agent_for_post == "codex";

    spawn_resume_session_watcher(
        app.clone(),
        task_id.clone(),
        project_path_for_post,
        session_id_for_post,
        is_codex,
    );
    let force_idle = dispatcher_state.register_subprocess(
        &dispatcher_session_id,
        &task_id,
        &dispatcher_dispatch_id,
        &agent_for_post,
        &dispatcher_description,
    );
    let idle_cb: Option<Box<dyn Fn(String) + Send>> = {
        let app_idle = app.clone();
        let tid_idle = task_id.clone();
        Some(Box::new(move |output: String| {
            let _ = app_idle.emit(
                "dispatcher-subprocess-idle",
                serde_json::json!({
                    "task_id": tid_idle,
                    "output": output
                }),
            );
        }))
    };
    spawn_pty_reader(
        app.clone(),
        task_id.clone(),
        "agent-output",
        "task_id",
        PtyEmitMode::Batched {
            flush_interval: PTY_EMIT_FLUSH_INTERVAL,
            max_batch_bytes: PTY_EMIT_MAX_BATCH_BYTES,
        },
        reader,
        None,
        idle_cb,
        None,
        Some(force_idle),
    );
    tokio::spawn(spawn_exit_monitor(app, task_id, is_codex));

    Ok(())
}

#[tauri::command]
pub async fn send_input(app: AppHandle, task_id: String, data: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let tm = app.state::<TaskManager>();
        tm.write_to_pty(&task_id, data.as_bytes(), false)
    })
    .await
    .map_err(|e| format!("spawn_blocking 失败: {e}"))?
}

#[tauri::command]
pub async fn resize_pty(
    app: AppHandle,
    task_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let task_manager = app.state::<TaskManager>();
        task_manager.resize_registered_pty(
            &task_id,
            PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            },
        )
    })
    .await
    .map_err(|e| format!("spawn_blocking 失败: {e}"))?
}

#[tauri::command]
pub async fn get_pty_output_snapshot(
    task_manager: State<'_, TaskManager>,
    task_id: String,
) -> Result<ManagedPtySnapshot, String> {
    task_manager.output_snapshot(&task_id)
}

#[tauri::command]
pub async fn open_shell(
    app: AppHandle,
    task_manager: State<'_, TaskManager>,
    shell_id: String,
    project_path: String,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<(), String> {
    // 先终止已存在的同 ID Shell
    {
        let _ = task_manager.kill_child(&shell_id);
        task_manager.remove_pty_handles(&shell_id);
    }

    let (master, writer, child, reader) = tokio::task::spawn_blocking(move || {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: rows.unwrap_or(24),
                cols: cols.unwrap_or(120),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let mut cmd = CommandBuilder::new(&shell);
        cmd.cwd(&project_path);
        setup_env(&mut cmd);

        let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

        Ok::<_, String>((pair.master, writer, child, reader))
    })
    .await
    .map_err(|e| format!("spawn_blocking 失败: {e}"))??;

    task_manager.insert_shell_pty_handles(&shell_id, master, writer, child);

    // Shell 退出后清理 TaskManager 中的残留句柄
    let app_cleanup = app.clone();
    let sid_cleanup = shell_id.clone();
    let on_finish = Box::new(move || {
        let tm = app_cleanup.state::<TaskManager>();
        tm.remove_pty_handles(&sid_cleanup);
    });

    spawn_pty_reader(
        app,
        shell_id,
        "shell-output",
        "shell_id",
        PtyEmitMode::Batched {
            flush_interval: PTY_EMIT_FLUSH_INTERVAL,
            max_batch_bytes: PTY_EMIT_MAX_BATCH_BYTES,
        },
        reader,
        None,
        None,
        Some(on_finish),
        None,
    );

    Ok(())
}

#[tauri::command]
pub async fn kill_shell(
    task_manager: State<'_, TaskManager>,
    shell_id: String,
) -> Result<(), String> {
    let _ = task_manager.kill_child(&shell_id);
    task_manager.remove_pty_handles(&shell_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{task_exit_failure_reason, task_exit_status, termination_status_label};
    use crate::shared::TaskTerminationIntent;

    #[test]
    fn termination_status_label_maps_stopped() {
        assert_eq!(
            termination_status_label(TaskTerminationIntent::Stopped),
            "stopped"
        );
    }

    #[test]
    fn nonzero_agent_exit_is_failed_even_when_session_exists() {
        assert_eq!(task_exit_status(false), "failed");
    }

    #[test]
    fn task_exit_failure_reason_keeps_exit_code_visible() {
        assert_eq!(
            task_exit_failure_reason(Some(42), None),
            "Agent 进程以非 0 状态退出，退出码：42"
        );
    }

    #[test]
    fn task_exit_failure_reason_keeps_wait_error_visible() {
        assert_eq!(
            task_exit_failure_reason(None, Some("wait failed")),
            "读取 Agent 进程退出状态失败：wait failed"
        );
    }
}

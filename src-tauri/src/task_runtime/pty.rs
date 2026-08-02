use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::Read;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::platform::get_login_shell_env;
use crate::shared::{ManagedPtySnapshot, TaskManager};

const PTY_READ_BUFFER_SIZE: usize = 64 * 1024;
const PTY_EMIT_FLUSH_INTERVAL: Duration = Duration::from_millis(16);
const PTY_EMIT_MAX_BATCH_BYTES: usize = 64 * 1024;
/// 有界 channel 容量：满时 reader 线程阻塞，反压传播至 OS 内核 PTY 缓冲区，
/// 最终使写入进程的 write() 系统调用阻塞，从源头限流。
const PTY_EMIT_CHANNEL_CAPACITY: usize = 1024;

// ── 共享 PTY 辅助函数 ────────────────────────────────────────────────────────

/// 设置 CommandBuilder 的标准环境变量。
fn setup_env(cmd: &mut CommandBuilder) {
    for (key, value) in get_login_shell_env() {
        cmd.env(key, value);
    }
    // 设置终端类型，使 Shell 输出正确的转义序列
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

/// 在后台线程中读取 PTY 输出，向前端发送事件。
///
/// - `event_name`：Tauri 事件名（当前为 `"shell-output"`）
/// - `id_key`：JSON payload 中的 ID 字段名（当前为 `"shell_id"`）
/// - `on_finish`：PTY 关闭后执行的可选清理回调
fn spawn_pty_reader(
    app: AppHandle,
    id: String,
    event_name: &'static str,
    id_key: &'static str,
    emit_mode: PtyEmitMode,
    reader: Box<dyn Read + Send>,
    on_finish: Option<Box<dyn FnOnce() + Send>>,
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
            loop {
                match rx.recv_timeout(flush_interval) {
                    Ok(chunk) => {
                        batch.push_str(&chunk);
                        if batch.len() >= max_batch_bytes {
                            flush_pty_batch(&emit_app, &emit_id, event_name, id_key, &mut batch);
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        flush_pty_batch(&emit_app, &emit_id, event_name, id_key, &mut batch);
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
        if let Some(f) = on_finish {
            f();
        }
    });
}

// ── Tauri 命令 ───────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn stop_task(
    task_manager: State<'_, TaskManager>,
    task_id: String,
) -> Result<(), String> {
    task_manager.kill_child(&task_id)
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
        Some(on_finish),
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

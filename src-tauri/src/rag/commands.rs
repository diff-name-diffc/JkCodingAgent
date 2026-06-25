//! Tauri 命令层——前端通过 invoke 调用。
//!
//! 所有命令遵守 AGENTS.md：HTTP/进程 I/O 均在 tokio 运行时内异步执行，
//! 不阻塞 Tauri 主线程；Mutex 临界区内只做内存读写。

use serde_json::Value;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, State};

use super::config::{RagConfigStore, RagKbConfig};
use super::logs::{RagLogEntry, RagLogStore};
use super::manager::RagManager;
use super::transport::{err_to_string, no_port_error};

/// 启动 sidecar（若未启动）并返回运行状态。
#[tauri::command]
pub async fn rag_start(
    app: AppHandle,
    manager: State<'_, RagManager>,
    config_store: State<'_, RagConfigStore>,
) -> Result<Value, String> {
    let handle = manager
        .ensure_started(&app, &config_store)
        .await
        .map_err(err_to_string)?;
    Ok(serde_json::json!({
        "running": true,
        "port": handle.port,
    }))
}

/// 停止 sidecar。
#[tauri::command]
pub fn rag_stop(manager: State<'_, RagManager>) -> Result<(), String> {
    manager.stop();
    Ok(())
}

/// 查询 sidecar 状态（是否运行、端口）。
#[tauri::command]
pub fn rag_status(manager: State<'_, RagManager>) -> Result<Value, String> {
    Ok(serde_json::json!({
        "running": manager.is_running(),
        "port": manager.current().map(|h| h.port),
    }))
}

/// 读取当前知识库配置（不脱敏，调用方需自行注意 UI 展示）。
#[tauri::command]
pub fn rag_get_kb_config(config_store: State<'_, RagConfigStore>) -> Result<RagKbConfig, String> {
    config_store.get_or_load().map_err(err_to_string)
}

/// 保存知识库配置：落盘 + 更新内存 + 若 sidecar 在运行则推送 reload。
#[tauri::command]
pub async fn rag_save_kb_config(
    manager: State<'_, RagManager>,
    config_store: State<'_, RagConfigStore>,
    config: RagKbConfig,
) -> Result<Value, String> {
    // 1. 落盘（锁外 I/O）
    config.save().map_err(err_to_string)?;
    // 2. 更新内存快照
    config_store.replace(config.clone());
    // 3. 若 sidecar 在运行，热推送
    if let Some(handle) = manager.current() {
        handle
            .transport
            .reload_config(&config)
            .await
            .map_err(err_to_string)?;
    }
    Ok(serde_json::json!({ "saved": true, "reloaded": manager.is_running() }))
}

/// 代理调用 sidecar 的 GET /health（便于前端确认服务就绪）。
#[tauri::command]
pub async fn rag_health(manager: State<'_, RagManager>) -> Result<Value, String> {
    let handle = manager
        .current()
        .ok_or_else(|| err_to_string(no_port_error()))?;
    handle.transport.health().await.map_err(err_to_string)
}

/// 保存当前草稿配置，并交给 sidecar 测试 Qdrant。
///
/// 约束：配置权威存储在桌面端；测试动作在无状态 sidecar 内完成。
#[tauri::command]
pub async fn rag_test_qdrant(
    app: AppHandle,
    manager: State<'_, RagManager>,
    config_store: State<'_, RagConfigStore>,
    config: RagKbConfig,
) -> Result<Value, String> {
    save_rag_config(&config_store, &config)?;
    let handle = manager
        .ensure_started(&app, &config_store)
        .await
        .map_err(err_to_string)?;
    handle
        .transport
        .test_qdrant(&config)
        .await
        .map_err(err_to_string)
}

/// 保存当前草稿配置，并交给 sidecar 测试 Embedding。
///
/// 约束：配置权威存储在桌面端；测试动作在无状态 sidecar 内完成。
#[tauri::command]
pub async fn rag_test_embedding(
    app: AppHandle,
    manager: State<'_, RagManager>,
    config_store: State<'_, RagConfigStore>,
    config: RagKbConfig,
) -> Result<Value, String> {
    save_rag_config(&config_store, &config)?;
    let handle = manager
        .ensure_started(&app, &config_store)
        .await
        .map_err(err_to_string)?;
    handle
        .transport
        .test_embedding(&config)
        .await
        .map_err(err_to_string)
}

/// 代理调用 sidecar 的 GET /config（脱敏视图）。
#[tauri::command]
pub async fn rag_sidecar_config(manager: State<'_, RagManager>) -> Result<Value, String> {
    let handle = manager
        .current()
        .ok_or_else(|| err_to_string(no_port_error()))?;
    handle.transport.get_config().await.map_err(err_to_string)
}

/// 返回当前内存中的 RAG sidecar 滚动日志。
#[tauri::command]
pub fn rag_logs_snapshot(log_store: State<'_, RagLogStore>) -> Result<Vec<RagLogEntry>, String> {
    Ok(log_store.snapshot())
}

/// 清空当前内存日志窗口。
#[tauri::command]
pub fn rag_logs_clear(log_store: State<'_, RagLogStore>) -> Result<(), String> {
    log_store.clear();
    Ok(())
}

/// 校验文件路径后启动 RAG 导入任务。
#[tauri::command]
pub async fn rag_ingest_files(
    app: AppHandle,
    manager: State<'_, RagManager>,
    config_store: State<'_, RagConfigStore>,
    project_id: String,
    project_path: String,
    files: Vec<String>,
) -> Result<Value, String> {
    let validated_files = validate_ingest_paths(project_path.clone(), files).await?;
    let handle = manager
        .ensure_started(&app, &config_store)
        .await
        .map_err(err_to_string)?;
    handle
        .transport
        .start_ingest_job(&project_id, &project_path, &validated_files)
        .await
        .map_err(err_to_string)
}

/// 查询 RAG 导入任务状态。
#[tauri::command]
pub async fn rag_ingest_job_status(
    manager: State<'_, RagManager>,
    job_id: String,
) -> Result<Value, String> {
    let handle = manager
        .current()
        .ok_or_else(|| err_to_string(no_port_error()))?;
    handle
        .transport
        .ingest_job_status(&job_id)
        .await
        .map_err(err_to_string)
}

fn save_rag_config(
    config_store: &State<'_, RagConfigStore>,
    config: &RagKbConfig,
) -> Result<(), String> {
    config.save().map_err(err_to_string)?;
    config_store.replace(config.clone());
    Ok(())
}

async fn validate_ingest_paths(
    project_path: String,
    files: Vec<String>,
) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if files.is_empty() {
            return Err("files 不能为空".to_string());
        }
        let root = canonical_dir(&project_path)?;
        files
            .into_iter()
            .map(|file| {
                let path = PathBuf::from(&file);
                if !path.is_absolute() {
                    return Err("文件路径必须是绝对路径".to_string());
                }
                let canonical = path
                    .canonicalize()
                    .map_err(|error| format!("无法解析文件路径 `{file}`：{error}"))?;
                if !canonical.is_file() {
                    return Err(format!("不是可导入文件：{}", canonical.display()));
                }
                if !canonical.starts_with(&root) {
                    return Err(format!("文件不在项目目录内：{}", canonical.display()));
                }
                Ok(canonical.to_string_lossy().into_owned())
            })
            .collect()
    })
    .await
    .map_err(|error| format!("校验导入路径失败：{error}"))?
}

fn canonical_dir(path: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(path);
    if !candidate.is_absolute() {
        return Err("projectPath 必须是绝对路径".to_string());
    }
    let canonical = candidate
        .canonicalize()
        .map_err(|error| format!("无法解析项目目录 `{path}`：{error}"))?;
    if !canonical.is_dir() {
        return Err(format!("projectPath 不是目录：{}", canonical.display()));
    }
    Ok(canonical)
}

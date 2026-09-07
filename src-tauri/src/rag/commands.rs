//! Tauri 命令层——前端通过 invoke 调用。
//!
//! 所有命令遵守 AGENTS.md：HTTP/进程 I/O 均在 tokio 运行时内异步执行，
//! 不阻塞 Tauri 主线程；Mutex 临界区内只做内存读写。

use anyhow::Context;
use serde_json::Value;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, State};

use super::config::{RagConfigStore, RagKbConfig};
use super::logs::{RagLogEntry, RagLogStore};
use super::manager::RagManager;
use super::transport::no_port_error;
use crate::shared::error::{CommandResult, IntoCommandResult};

type RagCommandResult<T> = std::result::Result<T, RagCommandError>;

#[derive(Debug, thiserror::Error)]
pub enum RagCommandError {
    #[error("files 不能为空")]
    EmptyFiles,
    #[error("文件路径必须是绝对路径")]
    FilePathNotAbsolute,
    #[error("projectPath 必须是绝对路径")]
    ProjectPathNotAbsolute,
    #[error("不是可导入文件：{0}")]
    NotFile(PathBuf),
    #[error("文件不在项目目录内：{0}")]
    OutsideProject(PathBuf),
    #[error("projectPath 不是目录：{0}")]
    ProjectPathNotDirectory(PathBuf),
    #[error("{action} 失败（{path}）：{source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("后台 RAG 任务失败：{0}")]
    TauriJoin(#[from] tauri::Error),
}

fn io_error(
    action: &'static str,
    path: impl Into<PathBuf>,
) -> impl FnOnce(std::io::Error) -> RagCommandError {
    move |source| RagCommandError::Io {
        action,
        path: path.into(),
        source,
    }
}

/// `rag_status` / `rag_restart` 返回 DTO（UI-22c 遗留登记）：内联透出最近
/// 失败原因，替代「未运行时只能翻服务日志」。camelCase 对齐前端
/// `types/rag.ts` 的 `RagRuntimeStatus`。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RagRuntimeStatus {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error_at: Option<i64>,
}

/// 原子重启 sidecar：在同一把 spawn 锁内完成 stop + spawn，
/// 避免前端两次 invoke 之间插入其他调用产生竞态或孤儿进程。
#[tauri::command]
pub async fn rag_restart(
    app: AppHandle,
    manager: State<'_, RagManager>,
    config_store: State<'_, RagConfigStore>,
) -> CommandResult<RagRuntimeStatus> {
    rag_restart_impl(app, manager, config_store)
        .await
        .context("重启 RAG sidecar 失败")
        .into_command_result()
}

async fn rag_restart_impl(
    app: AppHandle,
    manager: State<'_, RagManager>,
    config_store: State<'_, RagConfigStore>,
) -> anyhow::Result<RagRuntimeStatus> {
    let handle = manager
        .restart(&app, config_store.inner())
        .await
        .context("restart RAG sidecar")?;
    // 重启成功即无失败原因（manager 已在登记句柄时清空）。
    Ok(RagRuntimeStatus {
        running: true,
        port: Some(handle.port),
        last_error: None,
        last_error_at: None,
    })
}

/// 查询 sidecar 状态（是否运行、端口、最近失败原因）。
/// 纯读命令：不得附带重启/探活等副作用（docs/tauri-commands.md 约束）。
#[tauri::command]
pub fn rag_status(manager: State<'_, RagManager>) -> CommandResult<RagRuntimeStatus> {
    let failure = manager.failure();
    Ok(RagRuntimeStatus {
        running: manager.is_running(),
        port: manager.current().map(|h| h.port),
        last_error: failure.as_ref().map(|f| f.message.clone()),
        last_error_at: failure.map(|f| f.at_ms),
    })
}

/// 读取当前知识库配置（不脱敏，调用方需自行注意 UI 展示）。
#[tauri::command]
pub fn rag_get_kb_config(config_store: State<'_, RagConfigStore>) -> CommandResult<RagKbConfig> {
    config_store
        .get_or_load()
        .context("读取 RAG 知识库配置失败")
        .into_command_result()
}

/// 保存知识库配置：写全局库 + 更新内存 + 若 sidecar 在运行则推送 reload。
///
/// 返回 `{ saved, reloaded, reloadError }`：DB 落库成功后 reload 失败不再使
/// 整个命令报错（避免前端误判为未保存），而是以 `reloaded: false` +
/// `reloadError` 告知热更新结果；sidecar 未运行时 `reloadError` 为 null。
#[tauri::command]
pub async fn rag_save_kb_config(
    state: State<'_, crate::agent::DispatcherState>,
    manager: State<'_, RagManager>,
    config_store: State<'_, RagConfigStore>,
    config: RagKbConfig,
) -> CommandResult<Value> {
    rag_save_kb_config_impl(state, manager, config_store, config)
        .await
        .context("保存 RAG 知识库配置失败")
        .into_command_result()
}

async fn rag_save_kb_config_impl(
    state: State<'_, crate::agent::DispatcherState>,
    manager: State<'_, RagManager>,
    config_store: State<'_, RagConfigStore>,
    config: RagKbConfig,
) -> anyhow::Result<Value> {
    // 1. 写全局库 app_config（阻塞线程池执行，不占 Tokio worker）
    let db = state.db().clone();
    let config_for_save = config.clone();
    tokio::task::spawn_blocking(move || config_for_save.save_to_db(&db))
        .await
        .context("保存 RAG 配置任务失败")?
        .context("save RAG config")?;
    // 2. 更新内存快照
    config_store.replace(config.clone());
    // 3. 若 sidecar 在运行，热推送；失败不使整体保存失败（DB 已落库），
    // 由返回值的 reloaded / reloadError 区分，重启应用后生效。
    let mut reloaded = false;
    let mut reload_error: Option<String> = None;
    if let Some(handle) = manager.current() {
        match handle.transport.reload_config(&config).await {
            Ok(_) => reloaded = true,
            Err(error) => reload_error = Some(format!("{error:#}")),
        }
    }
    Ok(serde_json::json!({
        "saved": true,
        "reloaded": reloaded,
        "reloadError": reload_error,
    }))
}

/// 交给 sidecar 测试连接（原 rag_test_qdrant / rag_test_embedding 合并）。
///
/// 约束：配置权威存储在桌面端；测试动作在无状态 sidecar 内完成。
/// **不做内部落库**——保存责任归调用方（前端测试前先 `rag_save_kb_config`
/// 持久化，与 `rag_ingest_files` 的责任划分一致）。
#[tauri::command]
pub async fn rag_test_connection(
    app: AppHandle,
    manager: State<'_, RagManager>,
    config_store: State<'_, RagConfigStore>,
    config: RagKbConfig,
    target: String,
) -> CommandResult<Value> {
    rag_test_connection_impl(app, manager, config_store, config, target)
        .await
        .context("测试 RAG 连接失败")
        .into_command_result()
}

async fn rag_test_connection_impl(
    app: AppHandle,
    manager: State<'_, RagManager>,
    config_store: State<'_, RagConfigStore>,
    config: RagKbConfig,
    target: String,
) -> anyhow::Result<Value> {
    let handle = manager
        .ensure_started(&app, &config_store)
        .await
        .context("ensure RAG sidecar started")?;
    match target.as_str() {
        "qdrant" => handle
            .transport
            .test_qdrant(&config)
            .await
            .context("POST RAG /test/qdrant"),
        "embedding" => handle
            .transport
            .test_embedding(&config)
            .await
            .context("POST RAG /test/embedding"),
        other => anyhow::bail!("未知的 RAG 测试目标：{other}（可选 qdrant / embedding）"),
    }
}

/// 返回当前内存中的 RAG sidecar 滚动日志。
#[tauri::command]
pub fn rag_logs_snapshot(log_store: State<'_, RagLogStore>) -> CommandResult<Vec<RagLogEntry>> {
    Ok(log_store.snapshot())
}

/// 清空当前内存日志窗口。
#[tauri::command]
pub fn rag_logs_clear(log_store: State<'_, RagLogStore>) -> CommandResult<()> {
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
) -> CommandResult<Value> {
    rag_ingest_files_impl(app, manager, config_store, project_id, project_path, files)
        .await
        .context("启动 RAG 文件导入失败")
        .into_command_result()
}

async fn rag_ingest_files_impl(
    app: AppHandle,
    manager: State<'_, RagManager>,
    config_store: State<'_, RagConfigStore>,
    project_id: String,
    project_path: String,
    files: Vec<String>,
) -> anyhow::Result<Value> {
    let validated_files = validate_ingest_paths(project_path.clone(), files)
        .await
        .context("校验导入文件路径失败")?;
    let handle = manager
        .ensure_started(&app, &config_store)
        .await
        .context("ensure RAG sidecar started")?;
    handle
        .transport
        .start_ingest_job(&project_id, &project_path, &validated_files)
        .await
        .context("POST RAG /ingest/jobs")
}

/// 查询 RAG 导入任务状态。
#[tauri::command]
pub async fn rag_ingest_job_status(
    manager: State<'_, RagManager>,
    job_id: String,
) -> CommandResult<Value> {
    rag_ingest_job_status_impl(manager, job_id.clone())
        .await
        .with_context(|| format!("查询 RAG 导入任务失败（job_id={job_id}）"))
        .into_command_result()
}

async fn rag_ingest_job_status_impl(
    manager: State<'_, RagManager>,
    job_id: String,
) -> anyhow::Result<Value> {
    let handle = manager.current().ok_or_else(no_port_error)?;
    handle
        .transport
        .ingest_job_status(&job_id)
        .await
        .with_context(|| format!("GET RAG /ingest/jobs/{job_id}"))
}

async fn validate_ingest_paths(
    project_path: String,
    files: Vec<String>,
) -> anyhow::Result<Vec<String>> {
    Ok(
        tauri::async_runtime::spawn_blocking(move || -> RagCommandResult<Vec<String>> {
            if files.is_empty() {
                return Err(RagCommandError::EmptyFiles);
            }
            let root = canonical_dir(&project_path)?;
            files
                .into_iter()
                .map(|file| {
                    let path = PathBuf::from(&file);
                    if !path.is_absolute() {
                        return Err(RagCommandError::FilePathNotAbsolute);
                    }
                    let canonical = path
                        .canonicalize()
                        .map_err(io_error("解析导入文件路径", &path))?;
                    if !canonical.is_file() {
                        return Err(RagCommandError::NotFile(canonical));
                    }
                    if !canonical.starts_with(&root) {
                        return Err(RagCommandError::OutsideProject(canonical));
                    }
                    Ok(canonical.to_string_lossy().into_owned())
                })
                .collect()
        })
        .await??,
    )
}

fn canonical_dir(path: &str) -> RagCommandResult<PathBuf> {
    let candidate = Path::new(path);
    if !candidate.is_absolute() {
        return Err(RagCommandError::ProjectPathNotAbsolute);
    }
    let canonical = candidate
        .canonicalize()
        .map_err(io_error("解析项目目录", candidate))?;
    if !canonical.is_dir() {
        return Err(RagCommandError::ProjectPathNotDirectory(canonical));
    }
    Ok(canonical)
}

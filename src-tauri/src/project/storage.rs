use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::shared::error::{CommandResult, IntoCommandResult};

type StorageResult<T> = std::result::Result<T, StorageError>;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("找不到用户主目录")]
    HomeDirMissing,
    #[error("{action} 失败（{path}）：{source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{action} 失败（{path}）：{source}")]
    Json {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

fn io_error(
    action: &'static str,
    path: impl Into<PathBuf>,
) -> impl FnOnce(std::io::Error) -> StorageError {
    move |source| StorageError::Io {
        action,
        path: path.into(),
        source,
    }
}

fn json_error(
    action: &'static str,
    path: impl Into<PathBuf>,
) -> impl FnOnce(serde_json::Error) -> StorageError {
    move |source| StorageError::Json {
        action,
        path: path.into(),
        source,
    }
}

// ── Data types (mirror TypeScript interfaces) ────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
    #[serde(rename = "lastOpenedAt")]
    pub last_opened_at: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Task {
    pub id: String,
    #[serde(rename = "projectId")]
    pub project_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub prompt: String,
    pub agent: String,
    #[serde(rename = "permissionMode")]
    pub permission_mode: String,
    pub status: String,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(
        rename = "attentionRequestedAt",
        skip_serializing_if = "Option::is_none"
    )]
    pub attention_requested_at: Option<i64>,
    #[serde(rename = "claudeSessionId", skip_serializing_if = "Option::is_none")]
    pub claude_session_id: Option<String>,
    #[serde(rename = "claudeSessionPath", skip_serializing_if = "Option::is_none")]
    pub claude_session_path: Option<String>,
    #[serde(rename = "codexSessionId", skip_serializing_if = "Option::is_none")]
    pub codex_session_id: Option<String>,
    #[serde(rename = "codexSessionPath", skip_serializing_if = "Option::is_none")]
    pub codex_session_path: Option<String>,
    #[serde(
        rename = "dispatcherSessionId",
        skip_serializing_if = "Option::is_none"
    )]
    pub dispatcher_session_id: Option<String>,
    #[serde(
        rename = "dispatcherDispatchId",
        skip_serializing_if = "Option::is_none"
    )]
    pub dispatcher_dispatch_id: Option<String>,
    #[serde(
        rename = "dispatcherDescription",
        skip_serializing_if = "Option::is_none"
    )]
    pub dispatcher_description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starred: Option<bool>,
    #[serde(rename = "failureReason", skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
}

// ── Path helpers ─────────────────────────────────────────────────────────────

fn app_data_dir() -> StorageResult<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or(StorageError::HomeDirMissing)?;
    Ok(home.join(".jkcodingagent"))
}

fn projects_path() -> StorageResult<PathBuf> {
    Ok(app_data_dir()?.join("projects.json"))
}

fn tasks_path(project_id: &str) -> StorageResult<PathBuf> {
    Ok(project_dir(project_id)?.join("tasks.json"))
}

fn project_dir(project_id: &str) -> StorageResult<PathBuf> {
    Ok(app_data_dir()?.join("projects").join(project_id))
}

fn ensure_app_data_dirs() -> StorageResult<()> {
    let dir = app_data_dir()?;
    fs::create_dir_all(&dir).map_err(io_error("创建应用数据目录", dir))
}

fn ensure_project_dir(project_id: &str) -> StorageResult<()> {
    let dir = project_dir(project_id)?;
    fs::create_dir_all(&dir).map_err(io_error("创建项目任务目录", dir))
}

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub fn load_projects() -> CommandResult<Vec<Project>> {
    load_projects_impl()
        .context("加载项目列表失败")
        .into_command_result()
}

fn load_projects_impl() -> StorageResult<Vec<Project>> {
    let path = projects_path()?;
    if !path.exists() {
        return Ok(vec![]);
    }
    let raw = fs::read_to_string(&path).map_err(io_error("读取项目列表", path.clone()))?;
    serde_json::from_str(&raw).map_err(json_error("解析项目列表", path))
}

#[tauri::command]
pub fn save_projects(projects: Vec<Project>) -> CommandResult<()> {
    save_projects_impl(projects)
        .context("保存项目列表失败")
        .into_command_result()
}

fn save_projects_impl(projects: Vec<Project>) -> StorageResult<()> {
    ensure_app_data_dirs()?;
    let path = projects_path()?;
    let raw = serde_json::to_string_pretty(&projects)
        .map_err(json_error("序列化项目列表", path.clone()))?;
    atomic_write(&projects_path()?, &raw)
}

#[tauri::command]
pub fn load_project_tasks(project_id: String) -> CommandResult<Vec<Task>> {
    load_project_tasks_impl(&project_id)
        .with_context(|| format!("加载项目任务失败（project_id={project_id}）"))
        .into_command_result()
}

fn load_project_tasks_impl(project_id: &str) -> StorageResult<Vec<Task>> {
    let path = tasks_path(&project_id)?;
    if !path.exists() {
        return Ok(vec![]);
    }
    let raw = fs::read_to_string(&path).map_err(io_error("读取项目任务", path.clone()))?;
    serde_json::from_str(&raw).map_err(json_error("解析项目任务", path))
}

#[tauri::command]
pub fn save_project_tasks(project_id: String, tasks: Vec<Task>) -> CommandResult<()> {
    save_project_tasks_impl(&project_id, tasks)
        .with_context(|| format!("保存项目任务失败（project_id={project_id}）"))
        .into_command_result()
}

fn save_project_tasks_impl(project_id: &str, tasks: Vec<Task>) -> StorageResult<()> {
    ensure_project_dir(&project_id)?;
    let path = tasks_path(&project_id)?;
    if tasks.is_empty() {
        // Remove the file if no tasks left
        if path.exists() {
            fs::remove_file(&path).map_err(io_error("删除空任务文件", path))?;
        }
        return Ok(());
    }
    let raw =
        serde_json::to_string_pretty(&tasks).map_err(json_error("序列化项目任务", path.clone()))?;
    atomic_write(&path, &raw)
}

// ── Atomic write (write to tmp then rename) ───────────────────────────────────

/// 原子写入：先写入唯一临时文件，再 rename 到目标路径。
/// 临时文件名包含 pid + 纳秒时间戳，避免并发写入时临时文件相互覆盖。
pub fn atomic_write(path: &Path, content: &str) -> StorageResult<()> {
    let uid = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = path.with_file_name(format!(".{file_name}.{uid}.tmp"));
    fs::write(&tmp, content).map_err(io_error("写入临时文件", tmp.clone()))?;
    fs::rename(&tmp, path).map_err(io_error("替换目标文件", path))
}

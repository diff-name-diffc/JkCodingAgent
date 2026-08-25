use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::agent::DispatcherState;
use crate::shared::error::{CommandResult, IntoCommandResult};

type StorageResult<T> = std::result::Result<T, StorageError>;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("{action} 失败（{path}）：{source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
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

// ── Tauri commands（projects 表为唯一权威源）────────────────────────────────

#[tauri::command]
pub fn load_projects(state: tauri::State<'_, DispatcherState>) -> CommandResult<Vec<Project>> {
    state
        .db()
        .list_projects()
        .context("加载项目列表失败")
        .into_command_result()
}

#[tauri::command]
pub fn save_projects(
    state: tauri::State<'_, DispatcherState>,
    projects: Vec<Project>,
) -> CommandResult<()> {
    state
        .db()
        .save_projects_all(&projects)
        .context("保存项目列表失败")
        .into_command_result()
}

/// `project_delete` 的返回值：随删除结果带出被级联删除的会话 id，
/// 供前端清理模块级内存 store 中这些会话的残留状态。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDeleteResult {
    pub deleted_session_ids: Vec<String>,
}

/// 删除项目：级联清理该项目全部会话（DB 记录 + 聊天图片文件 + 工具产物）
/// 与项目仓库内应用自有的运行期数据目录（browser-profile / local_env）。
/// config.toml / mcp.json 可能随仓库共享，保留不删。
#[tauri::command]
pub async fn project_delete(
    state: tauri::State<'_, DispatcherState>,
    project_id: String,
) -> CommandResult<ProjectDeleteResult> {
    let db = state.db().clone();
    let result = tokio::task::spawn_blocking(move || {
        let plan = db.delete_project(&project_id)?;
        crate::agent::db::projects::cleanup_project_files(&plan);
        anyhow::Ok(ProjectDeleteResult {
            deleted_session_ids: plan.deleted_session_ids,
        })
    })
    .await
    .context("删除项目任务失败")
    .and_then(|inner| inner.context("删除项目失败"));
    result.into_command_result()
}

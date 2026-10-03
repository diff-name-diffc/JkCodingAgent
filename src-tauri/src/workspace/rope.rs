use anyhow::Context;
use parking_lot::Mutex;
use ropey::Rope;
use std::collections::HashMap;
use std::path::PathBuf;

use super::validate_path_within;

use crate::shared::error::{CommandResult, IntoCommandResult};

type RopeResult<T> = std::result::Result<T, RopeError>;

#[derive(Debug, thiserror::Error)]
pub enum RopeError {
    #[error("路径必须是绝对路径")]
    PathNotAbsolute,
    #[error("路径不在允许目录内")]
    OutsideAllowedDirectory,
    #[error("Rope session 不存在：{0}")]
    SessionNotFound(String),
    #[error("{action} 失败（{path}）：{source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("后台 Rope 任务失败：{0}")]
    TauriJoin(#[from] tauri::Error),
}

impl crate::shared::io_error::PathIoError for RopeError {
    fn path_io(action: &'static str, path: PathBuf, source: std::io::Error) -> Self {
        RopeError::Io {
            action,
            path,
            source,
        }
    }
}

/// io 错误闭包的类型钉住适配器：构造逻辑在 `shared::io_error`，此处固定
/// 目标错误类型（`?` 经 From 转换的调用点无法唯一推断泛型 E）。
fn io_error(
    action: &'static str,
    path: impl Into<PathBuf>,
) -> impl FnOnce(std::io::Error) -> RopeError {
    crate::shared::io_error::io_error(action, path)
}

impl From<super::PathValidationError> for RopeError {
    fn from(error: super::PathValidationError) -> Self {
        match error {
            super::PathValidationError::NotAbsolute => RopeError::PathNotAbsolute,
            super::PathValidationError::OutsideAllowed => RopeError::OutsideAllowedDirectory,
            super::PathValidationError::Io {
                action,
                path,
                source,
            } => RopeError::Io {
                action,
                path,
                source,
            },
        }
    }
}

/// Strip trailing `\r\n` or `\n` in-place — avoids the double allocation of
/// `.trim_end_matches('\n').trim_end_matches('\r').to_string()`.
fn strip_trailing_newline(s: &mut String) {
    let len = s.len();
    if len > 0 && s.as_bytes()[len - 1] == b'\n' {
        let cut = if len > 1 && s.as_bytes()[len - 2] == b'\r' {
            len - 2
        } else {
            len - 1
        };
        s.truncate(cut);
    }
}

/// Manages in-memory Rope read-only sessions keyed by file-viewer tabs.
pub struct RopeManager {
    sessions: Mutex<HashMap<String, RopeSession>>,
}

/// 只读会话：大文件查看器降级只读后仅承载切片读取，不再有编辑/撤销/保存状态。
struct RopeSession {
    path: PathBuf,
    rope: Rope,
}

impl RopeSession {
    fn meta(&self) -> RopeMeta {
        RopeMeta {
            line_count: self.rope.len_lines() as u64,
            char_count: self.rope.len_chars() as u64,
            byte_len: self.rope.len_bytes() as u64,
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RopeMeta {
    pub line_count: u64,
    pub char_count: u64,
    pub byte_len: u64,
}

impl RopeManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }
}

#[tauri::command]
pub async fn rope_open(
    state: tauri::State<'_, RopeManager>,
    session_id: String,
    path: String,
    project_path: String,
) -> CommandResult<RopeMeta> {
    rope_open_impl(state, session_id, path, project_path)
        .await
        .context("打开 Rope 会话失败")
        .into_command_result()
}

async fn rope_open_impl(
    state: tauri::State<'_, RopeManager>,
    session_id: String,
    path: String,
    project_path: String,
) -> RopeResult<RopeMeta> {
    let validated_path = validate_path_within(&path, &project_path)?;

    {
        let mut sessions = state.sessions.lock();
        if let Some(existing) = sessions.get(&session_id) {
            if existing.path == validated_path {
                return Ok(existing.meta());
            }
        }
        sessions.remove(&session_id);
    }

    let path_for_read = validated_path.clone();
    let rope = tauri::async_runtime::spawn_blocking(move || -> RopeResult<Rope> {
        let file = std::fs::File::open(&path_for_read)
            .map_err(io_error("打开 Rope 文件", &path_for_read))?;
        let reader = std::io::BufReader::with_capacity(256 * 1024, file);
        Rope::from_reader(reader).map_err(io_error("读取 Rope 文件", &path_for_read))
    })
    .await??;

    let session = RopeSession {
        path: validated_path,
        rope,
    };
    let meta = session.meta();

    state.sessions.lock().insert(session_id, session);

    Ok(meta)
}

#[tauri::command]
pub fn rope_read_lines(
    state: tauri::State<'_, RopeManager>,
    session_id: String,
    start_line: u64,
    max_lines: u64,
) -> CommandResult<Vec<String>> {
    rope_read_lines_impl(state, session_id, start_line, max_lines)
        .context("读取 Rope 行失败")
        .into_command_result()
}

fn rope_read_lines_impl(
    state: tauri::State<'_, RopeManager>,
    session_id: String,
    start_line: u64,
    max_lines: u64,
) -> RopeResult<Vec<String>> {
    let sessions = state.sessions.lock();
    let session = sessions
        .get(&session_id)
        .ok_or_else(|| RopeError::SessionNotFound(session_id.clone()))?;

    let total = session.rope.len_lines();
    let start = (start_line as usize).min(total);
    let end = ((start_line + max_lines) as usize).min(total);

    let mut lines = Vec::with_capacity(end.saturating_sub(start));
    for idx in start..end {
        let line = session.rope.line(idx);
        let mut line_text = line.to_string();
        strip_trailing_newline(&mut line_text);
        lines.push(line_text);
    }

    Ok(lines)
}

#[tauri::command]
pub fn rope_close(state: tauri::State<'_, RopeManager>, session_id: String) {
    state.sessions.lock().remove(&session_id);
}

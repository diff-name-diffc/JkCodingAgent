use anyhow::Context;
use parking_lot::Mutex;
use ropey::Rope;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::shared::error::{CommandResult, IntoCommandResult};

const MAX_UNDO_STACK: usize = 10;

type RopeResult<T> = std::result::Result<T, RopeError>;

#[derive(Debug, thiserror::Error)]
pub enum RopeError {
    #[error("路径必须是绝对路径")]
    PathNotAbsolute,
    #[error("路径不在允许目录内")]
    OutsideAllowedDirectory,
    #[error("Rope session 不存在：{0}")]
    SessionNotFound(String),
    #[error("行号越界：{line}，文件总行数：{total_lines}")]
    LineOutOfRange { line: u64, total_lines: usize },
    #[error("没有可撤销的编辑")]
    NothingToUndo,
    #[error("没有可重做的编辑")]
    NothingToRedo,
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

fn io_error(
    action: &'static str,
    path: impl Into<PathBuf>,
) -> impl FnOnce(std::io::Error) -> RopeError {
    move |source| RopeError::Io {
        action,
        path: path.into(),
        source,
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

/// Manages in-memory Rope edit sessions keyed by file-viewer tabs.
pub struct RopeManager {
    sessions: Mutex<HashMap<String, RopeSession>>,
}

struct RopeSession {
    path: PathBuf,
    rope: Rope,
    /// True if the rope has been modified since the last save.
    dirty: bool,
    /// Monotonic revision number for edit history bookkeeping.
    revision: u64,
    /// Revision number at the last successful save.
    saved_revision: u64,
    /// Undo stack: snapshots of the Rope before each committed edit.
    undo_stack: Vec<RopeSnapshot>,
    /// Redo stack: snapshots popped from undo that can be reapplied.
    redo_stack: Vec<RopeSnapshot>,
}

struct RopeSnapshot {
    rope: Rope,
    revision: u64,
}

impl RopeSession {
    fn meta(&self) -> RopeMeta {
        RopeMeta {
            line_count: self.rope.len_lines() as u64,
            char_count: self.rope.len_chars() as u64,
            byte_len: self.rope.len_bytes() as u64,
        }
    }

    fn push_undo_snapshot(&mut self) {
        if self.undo_stack.len() >= MAX_UNDO_STACK {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(RopeSnapshot {
            rope: self.rope.clone(),
            revision: self.revision,
        });
        self.redo_stack.clear();
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RopeMeta {
    pub line_count: u64,
    pub char_count: u64,
    pub byte_len: u64,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RopeEditResult {
    /// Total line count after the edit.
    pub line_count: u64,
    /// Lines that changed — frontend should re-fetch these.
    pub affected_start_line: u64,
    pub affected_end_line: u64,
}

impl RopeManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }
}

fn validate_path_within(target: &str, allowed_root: &str) -> RopeResult<PathBuf> {
    let target = Path::new(target);
    let root = Path::new(allowed_root);

    if !target.is_absolute() {
        return Err(RopeError::PathNotAbsolute);
    }

    let canonical_target = target
        .canonicalize()
        .map_err(io_error("解析目标路径", target))?;
    let canonical_root = root
        .canonicalize()
        .map_err(io_error("解析项目根目录", root))?;

    if !canonical_target.starts_with(&canonical_root) {
        return Err(RopeError::OutsideAllowedDirectory);
    }

    Ok(canonical_target)
}

fn ensure_session_in_project(session_path: &Path, project_path: &str) -> RopeResult<()> {
    let canonical_root = Path::new(project_path)
        .canonicalize()
        .map_err(io_error("解析项目根目录", project_path))?;

    if !session_path.starts_with(&canonical_root) {
        return Err(RopeError::OutsideAllowedDirectory);
    }

    Ok(())
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
        dirty: false,
        revision: 0,
        saved_revision: 0,
        rope,
        undo_stack: Vec::new(),
        redo_stack: Vec::new(),
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
pub fn rope_edit(
    state: tauri::State<'_, RopeManager>,
    session_id: String,
    line: u64,
    col: u64,
    delete_count: u64,
    insert_text: String,
) -> CommandResult<RopeEditResult> {
    rope_edit_impl(state, session_id, line, col, delete_count, insert_text)
        .context("编辑 Rope 内容失败")
        .into_command_result()
}

fn rope_edit_impl(
    state: tauri::State<'_, RopeManager>,
    session_id: String,
    line: u64,
    col: u64,
    delete_count: u64,
    insert_text: String,
) -> RopeResult<RopeEditResult> {
    let mut sessions = state.sessions.lock();
    let session = sessions
        .get_mut(&session_id)
        .ok_or_else(|| RopeError::SessionNotFound(session_id.clone()))?;

    let total_lines = session.rope.len_lines();
    let line_idx = (line as usize).min(total_lines.saturating_sub(1));
    let line_start_char = session.rope.line_to_char(line_idx);
    let line_len = session.rope.line(line_idx).len_chars();
    let col_clamped = (col as usize).min(line_len);
    let char_offset = line_start_char + col_clamped;
    let delete_end = (char_offset + delete_count as usize).min(session.rope.len_chars());
    let has_delete_effect = delete_end > char_offset;

    if !has_delete_effect && insert_text.is_empty() {
        let meta = session.meta();
        return Ok(RopeEditResult {
            line_count: meta.line_count,
            affected_start_line: line,
            affected_end_line: (line + 1).min(meta.line_count),
        });
    }

    session.push_undo_snapshot();

    if delete_count > 0 && delete_end > char_offset {
        session.rope.remove(char_offset..delete_end);
    }

    if !insert_text.is_empty() {
        let insert_at = char_offset.min(session.rope.len_chars());
        session.rope.insert(insert_at, &insert_text);
    }

    session.revision += 1;
    session.dirty = session.revision != session.saved_revision;

    let new_total = session.rope.len_lines() as u64;
    let newline_count = insert_text.chars().filter(|&ch| ch == '\n').count() as u64;

    Ok(RopeEditResult {
        line_count: new_total,
        affected_start_line: line,
        affected_end_line: (line + 1 + newline_count).min(new_total),
    })
}

#[tauri::command]
pub fn rope_replace_line(
    state: tauri::State<'_, RopeManager>,
    session_id: String,
    line: u64,
    new_content: String,
) -> CommandResult<RopeEditResult> {
    rope_replace_line_impl(state, session_id, line, new_content)
        .context("替换 Rope 行失败")
        .into_command_result()
}

fn rope_replace_line_impl(
    state: tauri::State<'_, RopeManager>,
    session_id: String,
    line: u64,
    new_content: String,
) -> RopeResult<RopeEditResult> {
    let mut sessions = state.sessions.lock();
    let session = sessions
        .get_mut(&session_id)
        .ok_or_else(|| RopeError::SessionNotFound(session_id.clone()))?;

    let total_lines = session.rope.len_lines();
    let line_idx = line as usize;
    if line_idx >= total_lines {
        return Err(RopeError::LineOutOfRange { line, total_lines });
    }

    let line_start = session.rope.line_to_char(line_idx);
    let old_line = session.rope.line(line_idx);
    let old_len = old_line.len_chars();
    let old_line_text = old_line
        .to_string()
        .trim_end_matches('\n')
        .trim_end_matches('\r')
        .to_string();

    if old_line_text == new_content {
        return Ok(RopeEditResult {
            line_count: total_lines as u64,
            affected_start_line: line,
            affected_end_line: (line + 1).min(total_lines as u64),
        });
    }

    let has_newline = old_len > 0 && {
        let last_char_idx = line_start + old_len - 1;
        session.rope.char(last_char_idx) == '\n'
    };

    session.push_undo_snapshot();

    let remove_end = if has_newline {
        line_start + old_len - 1
    } else {
        line_start + old_len
    };

    if remove_end > line_start {
        session.rope.remove(line_start..remove_end);
    }

    if !new_content.is_empty() {
        session.rope.insert(line_start, &new_content);
    }

    session.revision += 1;
    session.dirty = session.revision != session.saved_revision;

    let new_total = session.rope.len_lines() as u64;
    let newline_count = new_content.chars().filter(|&ch| ch == '\n').count() as u64;

    Ok(RopeEditResult {
        line_count: new_total,
        affected_start_line: line,
        affected_end_line: (line + 1 + newline_count).min(new_total),
    })
}

#[tauri::command]
pub async fn rope_save(
    state: tauri::State<'_, RopeManager>,
    session_id: String,
    project_path: String,
) -> CommandResult<()> {
    rope_save_impl(state, session_id, project_path)
        .await
        .context("保存 Rope 会话失败")
        .into_command_result()
}

async fn rope_save_impl(
    state: tauri::State<'_, RopeManager>,
    session_id: String,
    project_path: String,
) -> RopeResult<()> {
    let (path, rope_clone) = {
        let sessions = state.sessions.lock();
        let session = sessions
            .get(&session_id)
            .ok_or_else(|| RopeError::SessionNotFound(session_id.clone()))?;
        ensure_session_in_project(&session.path, &project_path)?;
        (session.path.clone(), session.rope.clone())
    };

    tauri::async_runtime::spawn_blocking(move || -> RopeResult<()> {
        let file = std::fs::File::create(&path).map_err(io_error("创建 Rope 文件", &path))?;
        let writer = std::io::BufWriter::with_capacity(256 * 1024, file);
        rope_clone
            .write_to(writer)
            .map_err(io_error("写入 Rope 文件", &path))
    })
    .await??;

    if let Some(session) = state.sessions.lock().get_mut(&session_id) {
        session.saved_revision = session.revision;
        session.dirty = false;
    }

    Ok(())
}

#[tauri::command]
pub fn rope_close(state: tauri::State<'_, RopeManager>, session_id: String) {
    state.sessions.lock().remove(&session_id);
}

#[tauri::command]
pub fn rope_undo(
    state: tauri::State<'_, RopeManager>,
    session_id: String,
) -> CommandResult<RopeMeta> {
    rope_undo_impl(state, session_id)
        .context("撤销 Rope 编辑失败")
        .into_command_result()
}

fn rope_undo_impl(
    state: tauri::State<'_, RopeManager>,
    session_id: String,
) -> RopeResult<RopeMeta> {
    let mut sessions = state.sessions.lock();
    let session = sessions
        .get_mut(&session_id)
        .ok_or_else(|| RopeError::SessionNotFound(session_id.clone()))?;

    let snapshot = session.undo_stack.pop().ok_or(RopeError::NothingToUndo)?;

    session.redo_stack.push(RopeSnapshot {
        rope: session.rope.clone(),
        revision: session.revision,
    });
    session.rope = snapshot.rope;
    session.revision = snapshot.revision;
    session.dirty = session.revision != session.saved_revision;

    Ok(session.meta())
}

#[tauri::command]
pub fn rope_redo(
    state: tauri::State<'_, RopeManager>,
    session_id: String,
) -> CommandResult<RopeMeta> {
    rope_redo_impl(state, session_id)
        .context("重做 Rope 编辑失败")
        .into_command_result()
}

fn rope_redo_impl(
    state: tauri::State<'_, RopeManager>,
    session_id: String,
) -> RopeResult<RopeMeta> {
    let mut sessions = state.sessions.lock();
    let session = sessions
        .get_mut(&session_id)
        .ok_or_else(|| RopeError::SessionNotFound(session_id.clone()))?;

    let snapshot = session.redo_stack.pop().ok_or(RopeError::NothingToRedo)?;

    if session.undo_stack.len() >= MAX_UNDO_STACK {
        session.undo_stack.remove(0);
    }
    session.undo_stack.push(RopeSnapshot {
        rope: session.rope.clone(),
        revision: session.revision,
    });
    session.rope = snapshot.rope;
    session.revision = snapshot.revision;
    session.dirty = session.revision != session.saved_revision;

    Ok(session.meta())
}

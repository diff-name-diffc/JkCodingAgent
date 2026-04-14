use parking_lot::Mutex;
use ropey::Rope;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const MAX_UNDO_STACK: usize = 10;

/// Manages in-memory Rope edit sessions keyed by file-viewer tabs.
pub struct RopeManager {
    sessions: Mutex<HashMap<String, RopeSession>>,
}

struct RopeSession {
    path: PathBuf,
    rope: Rope,
    /// True if the rope has been modified since the last save.
    dirty: bool,
    /// Undo stack: snapshots of the Rope before each committed edit.
    undo_stack: Vec<Rope>,
    /// Redo stack: snapshots popped from undo that can be reapplied.
    redo_stack: Vec<Rope>,
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
        self.undo_stack.push(self.rope.clone());
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

fn validate_path_within(target: &str, allowed_root: &str) -> Result<PathBuf, String> {
    let target = Path::new(target);
    let root = Path::new(allowed_root);

    if !target.is_absolute() {
        return Err("Path must be absolute".to_string());
    }

    let canonical_target = target
        .canonicalize()
        .map_err(|e| format!("Cannot resolve path: {}", e))?;
    let canonical_root = root
        .canonicalize()
        .map_err(|e| format!("Cannot resolve root directory: {}", e))?;

    if !canonical_target.starts_with(&canonical_root) {
        return Err("Path is outside the allowed directory".to_string());
    }

    Ok(canonical_target)
}

fn ensure_session_in_project(session_path: &Path, project_path: &str) -> Result<(), String> {
    let canonical_root = Path::new(project_path)
        .canonicalize()
        .map_err(|e| format!("Cannot resolve root directory: {}", e))?;

    if !session_path.starts_with(&canonical_root) {
        return Err("Path is outside the allowed directory".to_string());
    }

    Ok(())
}

#[tauri::command]
pub async fn rope_open(
    state: tauri::State<'_, RopeManager>,
    session_id: String,
    path: String,
    project_path: String,
) -> Result<RopeMeta, String> {
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
    let rope = tauri::async_runtime::spawn_blocking(move || {
        let file = std::fs::File::open(&path_for_read).map_err(|e| e.to_string())?;
        let reader = std::io::BufReader::with_capacity(256 * 1024, file);
        Rope::from_reader(reader).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    let session = RopeSession {
        path: validated_path,
        dirty: false,
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
) -> Result<Vec<String>, String> {
    let sessions = state.sessions.lock();
    let session = sessions
        .get(&session_id)
        .ok_or_else(|| format!("Rope session not found: {}", session_id))?;

    let total = session.rope.len_lines();
    let start = (start_line as usize).min(total);
    let end = ((start_line + max_lines) as usize).min(total);

    let mut lines = Vec::with_capacity(end.saturating_sub(start));
    for idx in start..end {
        let line = session.rope.line(idx);
        let line_text = line.to_string();
        lines.push(
            line_text
                .trim_end_matches('\n')
                .trim_end_matches('\r')
                .to_string(),
        );
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
) -> Result<RopeEditResult, String> {
    let mut sessions = state.sessions.lock();
    let session = sessions
        .get_mut(&session_id)
        .ok_or_else(|| format!("Rope session not found: {}", session_id))?;

    if delete_count == 0 && insert_text.is_empty() {
        let meta = session.meta();
        return Ok(RopeEditResult {
            line_count: meta.line_count,
            affected_start_line: line,
            affected_end_line: (line + 1).min(meta.line_count),
        });
    }

    session.push_undo_snapshot();

    let total_lines = session.rope.len_lines();
    let line_idx = (line as usize).min(total_lines.saturating_sub(1));
    let line_start_char = session.rope.line_to_char(line_idx);
    let line_len = session.rope.line(line_idx).len_chars();
    let col_clamped = (col as usize).min(line_len);
    let char_offset = line_start_char + col_clamped;

    if delete_count > 0 {
        let delete_end = (char_offset + delete_count as usize).min(session.rope.len_chars());
        if delete_end > char_offset {
            session.rope.remove(char_offset..delete_end);
        }
    }

    if !insert_text.is_empty() {
        let insert_at = char_offset.min(session.rope.len_chars());
        session.rope.insert(insert_at, &insert_text);
    }

    session.dirty = true;

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
) -> Result<RopeEditResult, String> {
    let mut sessions = state.sessions.lock();
    let session = sessions
        .get_mut(&session_id)
        .ok_or_else(|| format!("Rope session not found: {}", session_id))?;

    let total_lines = session.rope.len_lines();
    let line_idx = line as usize;
    if line_idx >= total_lines {
        return Err(format!(
            "Line {} is out of range (file has {} lines)",
            line, total_lines
        ));
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

    session.dirty = true;

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
) -> Result<(), String> {
    let (path, rope_clone) = {
        let sessions = state.sessions.lock();
        let session = sessions
            .get(&session_id)
            .ok_or_else(|| format!("Rope session not found: {}", session_id))?;
        ensure_session_in_project(&session.path, &project_path)?;
        (session.path.clone(), session.rope.clone())
    };

    tauri::async_runtime::spawn_blocking(move || {
        let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
        let writer = std::io::BufWriter::with_capacity(256 * 1024, file);
        rope_clone.write_to(writer).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    if let Some(session) = state.sessions.lock().get_mut(&session_id) {
        session.dirty = false;
    }

    Ok(())
}

#[tauri::command]
pub fn rope_is_dirty(state: tauri::State<'_, RopeManager>, session_id: String) -> bool {
    state
        .sessions
        .lock()
        .get(&session_id)
        .map(|session| session.dirty)
        .unwrap_or(false)
}

#[tauri::command]
pub fn rope_close(state: tauri::State<'_, RopeManager>, session_id: String) {
    state.sessions.lock().remove(&session_id);
}

#[tauri::command]
pub fn rope_undo(
    state: tauri::State<'_, RopeManager>,
    session_id: String,
) -> Result<RopeMeta, String> {
    let mut sessions = state.sessions.lock();
    let session = sessions
        .get_mut(&session_id)
        .ok_or_else(|| format!("Rope session not found: {}", session_id))?;

    let snapshot = session
        .undo_stack
        .pop()
        .ok_or_else(|| "Nothing to undo".to_string())?;

    session.redo_stack.push(session.rope.clone());
    session.rope = snapshot;
    session.dirty = true;

    Ok(session.meta())
}

#[tauri::command]
pub fn rope_redo(
    state: tauri::State<'_, RopeManager>,
    session_id: String,
) -> Result<RopeMeta, String> {
    let mut sessions = state.sessions.lock();
    let session = sessions
        .get_mut(&session_id)
        .ok_or_else(|| format!("Rope session not found: {}", session_id))?;

    let snapshot = session
        .redo_stack
        .pop()
        .ok_or_else(|| "Nothing to redo".to_string())?;

    if session.undo_stack.len() >= MAX_UNDO_STACK {
        session.undo_stack.remove(0);
    }
    session.undo_stack.push(session.rope.clone());
    session.rope = snapshot;
    session.dirty = true;

    Ok(session.meta())
}

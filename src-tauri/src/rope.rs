use parking_lot::Mutex;
use ropey::Rope;
use std::collections::HashMap;
use std::path::Path;

const MAX_UNDO_STACK: usize = 100;

/// Manages in-memory Rope buffers for open files.
/// Any file (regardless of size) can be opened into a Rope for O(log N) editing.
pub struct RopeManager {
    buffers: Mutex<HashMap<String, RopeBuffer>>,
}

struct RopeBuffer {
    rope: Rope,
    /// True if the rope has been modified since the last save.
    dirty: bool,
    /// Undo stack: snapshots of the Rope before each edit.
    undo_stack: Vec<Rope>,
    /// Redo stack: snapshots popped from undo that can be reapplied.
    redo_stack: Vec<Rope>,
}

impl RopeBuffer {
    /// Push a snapshot of the current rope onto the undo stack before an edit.
    fn push_undo_snapshot(&mut self) {
        if self.undo_stack.len() >= MAX_UNDO_STACK {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(self.rope.clone());
        // Any new edit clears the redo stack
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
            buffers: Mutex::new(HashMap::new()),
        }
    }
}

/// Validate that `target` is an absolute path within `allowed_root`.
fn validate_path_within(target: &str, allowed_root: &str) -> Result<std::path::PathBuf, String> {
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

/// Open a file into a Rope buffer. If already open, returns existing meta.
#[tauri::command]
pub async fn rope_open(
    state: tauri::State<'_, RopeManager>,
    path: String,
    project_path: String,
) -> Result<RopeMeta, String> {
    let validated_path = validate_path_within(&path, &project_path)?;
    let key = path.clone();

    // Check if already open
    {
        let buffers = state.buffers.lock();
        if let Some(buf) = buffers.get(&key) {
            return Ok(RopeMeta {
                line_count: buf.rope.len_lines() as u64,
                char_count: buf.rope.len_chars() as u64,
                byte_len: buf.rope.len_bytes() as u64,
            });
        }
    }

    // Load from disk in a blocking thread
    let rope = tauri::async_runtime::spawn_blocking(move || {
        let file = std::fs::File::open(&validated_path).map_err(|e| e.to_string())?;
        let reader = std::io::BufReader::with_capacity(256 * 1024, file);
        Rope::from_reader(reader).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    let meta = RopeMeta {
        line_count: rope.len_lines() as u64,
        char_count: rope.len_chars() as u64,
        byte_len: rope.len_bytes() as u64,
    };

    state.buffers.lock().insert(
        key,
        RopeBuffer {
            rope,
            dirty: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        },
    );

    Ok(meta)
}

/// Read a range of lines from an open Rope buffer.
/// O(log N) per line access.
#[tauri::command]
pub fn rope_read_lines(
    state: tauri::State<'_, RopeManager>,
    path: String,
    start_line: u64,
    max_lines: u64,
) -> Result<Vec<String>, String> {
    let buffers = state.buffers.lock();
    let buf = buffers
        .get(&path)
        .ok_or_else(|| format!("File not open in rope: {}", path))?;

    let total = buf.rope.len_lines();
    let start = (start_line as usize).min(total);
    let end = ((start_line + max_lines) as usize).min(total);

    let mut lines = Vec::with_capacity(end - start);
    for i in start..end {
        let line = buf.rope.line(i);
        // Trim trailing newline for display
        let s = line.to_string();
        lines.push(s.trim_end_matches('\n').trim_end_matches('\r').to_string());
    }

    Ok(lines)
}

/// Apply an edit to an open Rope buffer.
/// `line` and `col` are 0-indexed. `delete_count` is in chars.
#[tauri::command]
pub fn rope_edit(
    state: tauri::State<'_, RopeManager>,
    path: String,
    line: u64,
    col: u64,
    delete_count: u64,
    insert_text: String,
) -> Result<RopeEditResult, String> {
    let mut buffers = state.buffers.lock();
    let buf = buffers
        .get_mut(&path)
        .ok_or_else(|| format!("File not open in rope: {}", path))?;

    buf.push_undo_snapshot();

    let total_lines = buf.rope.len_lines();
    let line_idx = (line as usize).min(total_lines.saturating_sub(1));

    // Convert (line, col) to char offset
    let line_start_char = buf.rope.line_to_char(line_idx);
    let line_len = buf.rope.line(line_idx).len_chars();
    let col_clamped = (col as usize).min(line_len);
    let char_offset = line_start_char + col_clamped;

    // Delete
    if delete_count > 0 {
        let del_end = (char_offset + delete_count as usize).min(buf.rope.len_chars());
        if del_end > char_offset {
            buf.rope.remove(char_offset..del_end);
        }
    }

    // Insert
    if !insert_text.is_empty() {
        let insert_at = char_offset.min(buf.rope.len_chars());
        buf.rope.insert(insert_at, &insert_text);
    }

    buf.dirty = true;

    // Calculate affected line range (for frontend refresh)
    let new_total = buf.rope.len_lines() as u64;
    let newline_count = insert_text.chars().filter(|&c| c == '\n').count() as u64;
    let affected_end = (line + 1 + newline_count).min(new_total);

    Ok(RopeEditResult {
        line_count: new_total,
        affected_start_line: line,
        affected_end_line: affected_end,
    })
}

/// Replace an entire line's content (convenience for contentEditable updates).
#[tauri::command]
pub fn rope_replace_line(
    state: tauri::State<'_, RopeManager>,
    path: String,
    line: u64,
    new_content: String,
) -> Result<RopeEditResult, String> {
    let mut buffers = state.buffers.lock();
    let buf = buffers
        .get_mut(&path)
        .ok_or_else(|| format!("File not open in rope: {}", path))?;

    buf.push_undo_snapshot();

    let total_lines = buf.rope.len_lines();
    let line_idx = line as usize;
    if line_idx >= total_lines {
        return Err(format!(
            "Line {} is out of range (file has {} lines)",
            line, total_lines
        ));
    }

    let line_start = buf.rope.line_to_char(line_idx);
    let old_line = buf.rope.line(line_idx);
    let old_len = old_line.len_chars();

    // Remove old line content (keep trailing \n if it exists)
    let has_newline = old_len > 0 && {
        let last_char_idx = line_start + old_len - 1;
        buf.rope.char(last_char_idx) == '\n'
    };

    let remove_end = if has_newline {
        line_start + old_len - 1 // keep the \n
    } else {
        line_start + old_len
    };

    if remove_end > line_start {
        buf.rope.remove(line_start..remove_end);
    }

    // Insert new content at line start
    if !new_content.is_empty() {
        buf.rope.insert(line_start, &new_content);
    }

    buf.dirty = true;

    let new_total = buf.rope.len_lines() as u64;
    let newline_count = new_content.chars().filter(|&c| c == '\n').count() as u64;

    Ok(RopeEditResult {
        line_count: new_total,
        affected_start_line: line,
        affected_end_line: (line + 1 + newline_count).min(new_total),
    })
}

/// Save the Rope buffer back to disk.
#[tauri::command]
pub async fn rope_save(
    state: tauri::State<'_, RopeManager>,
    path: String,
    project_path: String,
) -> Result<(), String> {
    let validated_path = validate_path_within(&path, &project_path)?;
    let key = path.clone();

    // Clone the rope data for writing (release lock quickly)
    let rope_clone = {
        let buffers = state.buffers.lock();
        let buf = buffers
            .get(&key)
            .ok_or_else(|| format!("File not open in rope: {}", key))?;
        buf.rope.clone()
    };

    tauri::async_runtime::spawn_blocking(move || {
        let file = std::fs::File::create(&validated_path).map_err(|e| e.to_string())?;
        let writer = std::io::BufWriter::with_capacity(256 * 1024, file);
        rope_clone.write_to(writer).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    // Mark as clean
    let mut buffers = state.buffers.lock();
    if let Some(buf) = buffers.get_mut(&key) {
        buf.dirty = false;
    }

    Ok(())
}

/// Check if a rope buffer has unsaved changes.
#[tauri::command]
pub fn rope_is_dirty(state: tauri::State<'_, RopeManager>, path: String) -> bool {
    state
        .buffers
        .lock()
        .get(&path)
        .map(|b| b.dirty)
        .unwrap_or(false)
}

/// Close a Rope buffer, releasing memory.
#[tauri::command]
pub fn rope_close(state: tauri::State<'_, RopeManager>, path: String) {
    state.buffers.lock().remove(&path);
}

/// Undo the last edit to a Rope buffer.
#[tauri::command]
pub fn rope_undo(
    state: tauri::State<'_, RopeManager>,
    path: String,
) -> Result<RopeMeta, String> {
    let mut buffers = state.buffers.lock();
    let buf = buffers
        .get_mut(&path)
        .ok_or_else(|| format!("File not open in rope: {}", path))?;

    let snapshot = buf
        .undo_stack
        .pop()
        .ok_or_else(|| "Nothing to undo".to_string())?;

    // Push current state onto redo stack
    buf.redo_stack.push(buf.rope.clone());
    buf.rope = snapshot;
    buf.dirty = true;

    Ok(RopeMeta {
        line_count: buf.rope.len_lines() as u64,
        char_count: buf.rope.len_chars() as u64,
        byte_len: buf.rope.len_bytes() as u64,
    })
}

/// Redo the last undone edit to a Rope buffer.
#[tauri::command]
pub fn rope_redo(
    state: tauri::State<'_, RopeManager>,
    path: String,
) -> Result<RopeMeta, String> {
    let mut buffers = state.buffers.lock();
    let buf = buffers
        .get_mut(&path)
        .ok_or_else(|| format!("File not open in rope: {}", path))?;

    let snapshot = buf
        .redo_stack
        .pop()
        .ok_or_else(|| "Nothing to redo".to_string())?;

    // Push current state onto undo stack
    buf.undo_stack.push(buf.rope.clone());
    buf.rope = snapshot;
    buf.dirty = true;

    Ok(RopeMeta {
        line_count: buf.rope.len_lines() as u64,
        char_count: buf.rope.len_chars() as u64,
        byte_len: buf.rope.len_bytes() as u64,
    })
}

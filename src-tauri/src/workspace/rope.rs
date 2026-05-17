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

    if delete_count > 0 {
        if delete_end > char_offset {
            session.rope.remove(char_offset..delete_end);
        }
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
        session.saved_revision = session.revision;
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
    session.undo_stack.push(RopeSnapshot {
        rope: session.rope.clone(),
        revision: session.revision,
    });
    session.rope = snapshot.rope;
    session.revision = snapshot.revision;
    session.dirty = session.revision != session.saved_revision;

    Ok(session.meta())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_test_dir(prefix: &str) -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
        std::env::temp_dir().join(format!("nezha_rope_{}_{}", prefix, id))
    }

    /// Create a unique temp directory. Cleaned up on drop.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let path = unique_test_dir(prefix);
            let _ = fs::create_dir_all(&path);
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// Helper to create a RopeManager with a session loaded from a temp file.
    struct TestRopeEnv {
        manager: RopeManager,
        _tmp: TempDir,
        file_path: PathBuf,
    }

    impl TestRopeEnv {
        fn new(content: &str) -> Self {
            let tmp = TempDir::new("rope_env");
            let file_path = tmp.path().join("test_file.txt");
            fs::write(&file_path, content).expect("write test file");

            let manager = RopeManager::new();
            let rope = Rope::from(content);
            let session = RopeSession {
                path: file_path.clone(),
                dirty: false,
                revision: 0,
                saved_revision: 0,
                rope,
                undo_stack: Vec::new(),
                redo_stack: Vec::new(),
            };
            manager.sessions.lock().insert("test-session".to_string(), session);

            Self {
                manager,
                _tmp: tmp,
                file_path,
            }
        }

        fn sessions(&self) -> parking_lot::MutexGuard<'_, HashMap<String, RopeSession>> {
            self.manager.sessions.lock()
        }
    }

    // ── RopeMeta tests ────────────────────────────────────────────────────

    #[test]
    fn rope_meta_counts_single_line() {
        let env = TestRopeEnv::new("hello world");
        let sessions = env.sessions();
        let s = sessions.get("test-session").unwrap();
        let meta = s.meta();
        assert_eq!(meta.line_count, 1);
        assert_eq!(meta.char_count, 11);
        assert_eq!(meta.byte_len, 11);
    }

    #[test]
    fn rope_meta_counts_multiline() {
        let env = TestRopeEnv::new("line1\nline2\nline3\n");
        let sessions = env.sessions();
        let s = sessions.get("test-session").unwrap();
        let meta = s.meta();
        assert_eq!(meta.line_count, 4); // 3 content lines + trailing newline creates 4th
        assert!(meta.char_count > 0);
    }

    #[test]
    fn rope_meta_empty_file() {
        let env = TestRopeEnv::new("");
        let sessions = env.sessions();
        let s = sessions.get("test-session").unwrap();
        let meta = s.meta();
        assert_eq!(meta.line_count, 1); // Empty rope has 1 line
        assert_eq!(meta.char_count, 0);
        assert_eq!(meta.byte_len, 0);
    }

    #[test]
    fn rope_meta_unicode_content() {
        let env = TestRopeEnv::new("中文测试");
        let sessions = env.sessions();
        let s = sessions.get("test-session").unwrap();
        let meta = s.meta();
        assert_eq!(meta.char_count, 4); // 4 CJK characters
        assert!(meta.byte_len > 4); // UTF-8 encoding is larger
    }

    // ── RopeSession dirty tracking ────────────────────────────────────────

    #[test]
    fn session_starts_not_dirty() {
        let env = TestRopeEnv::new("hello");
        let sessions = env.sessions();
        let s = sessions.get("test-session").unwrap();
        assert!(!s.dirty);
    }

    #[test]
    fn session_dirty_after_edit() {
        let env = TestRopeEnv::new("hello");
        let mut sessions = env.manager.sessions.lock();
        let s = sessions.get_mut("test-session").unwrap();

        s.push_undo_snapshot();
        s.rope.insert(0, "X");
        s.revision += 1;
        s.dirty = s.revision != s.saved_revision;

        assert!(s.dirty);
        assert_eq!(s.revision, 1);
    }

    #[test]
    fn session_not_dirty_after_save() {
        let env = TestRopeEnv::new("hello");
        let mut sessions = env.manager.sessions.lock();
        let s = sessions.get_mut("test-session").unwrap();

        s.push_undo_snapshot();
        s.rope.insert(0, "X");
        s.revision += 1;
        s.saved_revision = s.revision;
        s.dirty = s.revision != s.saved_revision;

        assert!(!s.dirty);
    }

    // ── Undo/Redo tests ──────────────────────────────────────────────────

    #[test]
    fn undo_restores_previous_state() {
        let env = TestRopeEnv::new("hello");
        let mut sessions = env.manager.sessions.lock();
        let s = sessions.get_mut("test-session").unwrap();
        assert_eq!(s.revision, 0);

        // Edit: insert "X" at start
        s.push_undo_snapshot();
        s.rope.insert(0, "X");
        s.revision = 1;
        s.dirty = true;
        assert_eq!(s.rope.to_string(), "Xhello");

        // Undo
        let snapshot = s.undo_stack.pop().unwrap();
        s.redo_stack.push(RopeSnapshot {
            rope: s.rope.clone(),
            revision: s.revision,
        });
        s.rope = snapshot.rope;
        s.revision = snapshot.revision;
        s.dirty = s.revision != s.saved_revision;

        assert_eq!(s.rope.to_string(), "hello");
        assert_eq!(s.revision, 0);
        assert!(!s.dirty);
    }

    #[test]
    fn redo_restores_undone_state() {
        let env = TestRopeEnv::new("hello");
        let mut sessions = env.manager.sessions.lock();
        let s = sessions.get_mut("test-session").unwrap();

        // Edit
        s.push_undo_snapshot();
        s.rope.insert(0, "X");
        s.revision = 1;

        // Undo
        let snapshot = s.undo_stack.pop().unwrap();
        s.redo_stack.push(RopeSnapshot {
            rope: s.rope.clone(),
            revision: s.revision,
        });
        s.rope = snapshot.rope;
        s.revision = snapshot.revision;

        assert_eq!(s.rope.to_string(), "hello");

        // Redo
        let redo_snapshot = s.redo_stack.pop().unwrap();
        s.undo_stack.push(RopeSnapshot {
            rope: s.rope.clone(),
            revision: s.revision,
        });
        s.rope = redo_snapshot.rope;
        s.revision = redo_snapshot.revision;

        assert_eq!(s.rope.to_string(), "Xhello");
        assert_eq!(s.revision, 1);
    }

    #[test]
    fn undo_stack_empty_when_fresh() {
        let env = TestRopeEnv::new("hello");
        let sessions = env.sessions();
        let s = sessions.get("test-session").unwrap();
        assert!(s.undo_stack.is_empty());
    }

    #[test]
    fn redo_stack_empty_when_fresh() {
        let env = TestRopeEnv::new("hello");
        let sessions = env.sessions();
        let s = sessions.get("test-session").unwrap();
        assert!(s.redo_stack.is_empty());
    }

    #[test]
    fn push_undo_clears_redo_stack() {
        let env = TestRopeEnv::new("hello");
        let mut sessions = env.manager.sessions.lock();
        let s = sessions.get_mut("test-session").unwrap();

        // First edit
        s.push_undo_snapshot();
        s.rope.insert(0, "A");
        s.revision = 1;

        // Simulate undo (pushes to redo)
        let snapshot = s.undo_stack.pop().unwrap();
        s.redo_stack.push(RopeSnapshot {
            rope: s.rope.clone(),
            revision: s.revision,
        });
        s.rope = snapshot.rope;
        s.revision = snapshot.revision;
        assert_eq!(s.redo_stack.len(), 1);

        // New edit should clear redo stack
        s.push_undo_snapshot();
        assert!(s.redo_stack.is_empty());
    }

    // ── Undo stack size limit ─────────────────────────────────────────────

    #[test]
    fn undo_stack_respects_max_size() {
        let env = TestRopeEnv::new("hello");
        let mut sessions = env.manager.sessions.lock();
        let s = sessions.get_mut("test-session").unwrap();

        // Push MAX_UNDO_STACK + 5 snapshots
        for i in 0..(MAX_UNDO_STACK + 5) {
            s.push_undo_snapshot();
            s.rope.insert(0, &format!("{}", i % 10));
            s.revision += 1;
        }

        assert_eq!(s.undo_stack.len(), MAX_UNDO_STACK);
    }

    // ── RopeManager basic operations ─────────────────────────────────────

    #[test]
    fn rope_manager_new_is_empty() {
        let mgr = RopeManager::new();
        assert!(mgr.sessions.lock().is_empty());
    }

    #[test]
    fn rope_manager_insert_and_retrieve_session() {
        let mgr = RopeManager::new();
        let rope = Rope::from("test content");
        let session = RopeSession {
            path: PathBuf::from("/tmp/test.txt"),
            dirty: false,
            revision: 0,
            saved_revision: 0,
            rope,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        };
        mgr.sessions.lock().insert("s1".to_string(), session);

        let sessions = mgr.sessions.lock();
        assert!(sessions.contains_key("s1"));
        let s = sessions.get("s1").unwrap();
        assert_eq!(s.rope.to_string(), "test content");
    }

    #[test]
    fn rope_manager_remove_session() {
        let mgr = RopeManager::new();
        let rope = Rope::from("test");
        let session = RopeSession {
            path: PathBuf::from("/tmp/test.txt"),
            dirty: false,
            revision: 0,
            saved_revision: 0,
            rope,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        };
        mgr.sessions.lock().insert("s1".to_string(), session);
        assert!(mgr.sessions.lock().contains_key("s1"));

        mgr.sessions.lock().remove("s1");
        assert!(!mgr.sessions.lock().contains_key("s1"));
    }

    // ── Line reading via rope ─────────────────────────────────────────────

    #[test]
    fn rope_line_iteration() {
        let rope = Rope::from("line1\nline2\nline3");
        assert_eq!(rope.len_lines(), 3);

        let line0: String = rope.line(0).to_string();
        assert_eq!(line0.trim_end_matches('\n'), "line1");

        let line1: String = rope.line(1).to_string();
        assert_eq!(line1.trim_end_matches('\n'), "line2");
    }

    #[test]
    fn rope_trailing_newline_creates_extra_line() {
        let rope = Rope::from("line1\nline2\n");
        // ropey counts the trailing newline as creating an extra empty line
        assert_eq!(rope.len_lines(), 3);
    }

    // ── validate_path_within tests ────────────────────────────────────────

    #[test]
    fn validate_path_within_rejects_relative_path() {
        let result = validate_path_within("relative/path", "/some/root");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("absolute"));
    }

    #[test]
    fn validate_path_within_rejects_nonexistent_path() {
        let result = validate_path_within("/nonexistent/path/file.txt", "/nonexistent/path");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("resolve"));
    }

    #[test]
    fn validate_path_within_accepts_valid_path() {
        let tmp = TempDir::new("rope_validate");
        let root = tmp.path();
        let file_path = root.join("test.txt");
        fs::write(&file_path, "content").expect("write file");

        let result = validate_path_within(
            file_path.to_str().unwrap(),
            root.to_str().unwrap(),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn validate_path_within_rejects_outside_root() {
        let tmp1 = TempDir::new("rope_validate_out1");
        let tmp2 = TempDir::new("rope_validate_out2");
        let file_path = tmp2.path().join("test.txt");
        fs::write(&file_path, "content").expect("write file");

        let result = validate_path_within(
            file_path.to_str().unwrap(),
            tmp1.path().to_str().unwrap(),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("outside"));
    }

    // ── ensure_session_in_project tests ────────────────────────────────────

    #[test]
    fn ensure_session_in_project_accepts_subpath() {
        let tmp = TempDir::new("rope_session");
        let root = tmp.path();
        let sub = root.join("subdir");
        fs::create_dir_all(&sub).expect("create subdir");

        // canonicalize both sides to handle macOS /tmp -> /private/tmp symlink
        let canonical_sub = sub.canonicalize().expect("canonicalize sub");
        let result = ensure_session_in_project(&canonical_sub, root.to_str().unwrap());
        assert!(result.is_ok());
    }

    #[test]
    fn ensure_session_in_project_rejects_outside() {
        let tmp1 = TempDir::new("rope_session_out1");
        let tmp2 = TempDir::new("rope_session_out2");

        let result = ensure_session_in_project(tmp1.path(), tmp2.path().to_str().unwrap());
        assert!(result.is_err());
    }

    // ── RopeSnapshot tracking ─────────────────────────────────────────────

    #[test]
    fn snapshot_preserves_revision() {
        let env = TestRopeEnv::new("hello");
        let mut sessions = env.manager.sessions.lock();
        let s = sessions.get_mut("test-session").unwrap();

        s.revision = 42;
        s.push_undo_snapshot();

        let snap = s.undo_stack.pop().unwrap();
        assert_eq!(snap.revision, 42);
    }

    #[test]
    fn multiple_edits_build_undo_stack() {
        let env = TestRopeEnv::new("abc");
        let mut sessions = env.manager.sessions.lock();
        let s = sessions.get_mut("test-session").unwrap();

        for i in 0..5 {
            s.push_undo_snapshot();
            s.rope.insert(s.rope.len_chars(), &format!("{}", i));
            s.revision += 1;
        }

        assert_eq!(s.undo_stack.len(), 5);
        assert_eq!(s.rope.to_string(), "abc01234");
    }
}

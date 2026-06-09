use parking_lot::Mutex;
use portable_pty::{Child, ExitStatus, MasterPty, PtySize};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::sync::Arc;

use crate::task_runtime::session::{ClaudeSessionInfo, CodexSessionInfo};

/// Lightweight cancellation token: clone-able, signal via `cancel()`, poll via `is_cancelled()`.
#[derive(Clone)]
pub(crate) struct CancellationToken {
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}

impl CancellationToken {
    pub(crate) fn new() -> Self {
        Self {
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub(crate) fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Release);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::Acquire)
    }
}

pub(crate) type SharedPtyMaster = Arc<Mutex<Box<dyn MasterPty + Send>>>;
pub(crate) type SharedPtyWriter = Arc<Mutex<Box<dyn Write + Send>>>;
pub(crate) type SharedChildHandle = Arc<Mutex<Box<dyn Child + Send + Sync>>>;

const SESSION_OUTPUT_MAX_BYTES: usize = 10 * 1024 * 1024;
const SESSION_OUTPUT_MAX_CHUNKS: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManagedPtyKind {
    Agent,
    Shell,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManagedPtyStatus {
    Running,
    Stopping,
    Finished,
    Released,
}

#[derive(Debug)]
struct SessionOutputBuffer {
    chunks: VecDeque<String>,
    bytes: usize,
    dropped_bytes: usize,
    seq: u64,
}

impl Default for SessionOutputBuffer {
    fn default() -> Self {
        Self {
            chunks: VecDeque::new(),
            bytes: 0,
            dropped_bytes: 0,
            seq: 0,
        }
    }
}

impl SessionOutputBuffer {
    fn push(&mut self, data: &str) -> u64 {
        self.seq += 1;
        self.bytes += data.len();
        self.chunks.push_back(data.to_string());

        while self.bytes > SESSION_OUTPUT_MAX_BYTES {
            let Some(chunk) = self.chunks.pop_front() else {
                self.bytes = 0;
                break;
            };
            self.bytes = self.bytes.saturating_sub(chunk.len());
            self.dropped_bytes += chunk.len();
        }

        if self.chunks.len() > SESSION_OUTPUT_MAX_CHUNKS {
            let merged = self.chunks.iter().map(String::as_str).collect::<String>();
            self.chunks.clear();
            self.chunks.push_back(merged);
        }

        self.seq
    }

    fn snapshot(&self) -> ManagedPtySnapshot {
        ManagedPtySnapshot {
            output: self.chunks.iter().map(String::as_str).collect::<String>(),
            seq: self.seq,
            dropped_bytes: self.dropped_bytes,
        }
    }
}

#[derive(Debug)]
struct ManagedPtySession {
    kind: ManagedPtyKind,
    status: ManagedPtyStatus,
    output: SessionOutputBuffer,
}

impl ManagedPtySession {
    fn new(kind: ManagedPtyKind) -> Self {
        Self {
            kind,
            status: ManagedPtyStatus::Running,
            output: SessionOutputBuffer::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ManagedPtySnapshot {
    pub output: String,
    pub seq: u64,
    pub dropped_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TaskTerminationIntent {
    Stopped,
}

#[derive(Default)]
pub struct TaskManager {
    pub(crate) pty_masters: Mutex<HashMap<String, SharedPtyMaster>>,
    pub(crate) pty_writers: Mutex<HashMap<String, SharedPtyWriter>>,
    pub(crate) child_handles: Mutex<HashMap<String, SharedChildHandle>>,
    pub(crate) task_termination_intents: Mutex<HashMap<String, TaskTerminationIntent>>,
    pub(crate) codex_sessions: Mutex<HashMap<String, CodexSessionInfo>>,
    pub(crate) claude_sessions: Mutex<HashMap<String, ClaudeSessionInfo>>,
    pub(crate) claimed_session_paths: Mutex<std::collections::HashSet<String>>,
    /// Cancellation tokens for session watcher threads — cancel on task cleanup.
    pub(crate) session_watchers: Mutex<HashMap<String, CancellationToken>>,
    managed_sessions: Mutex<HashMap<String, ManagedPtySession>>,
}

impl TaskManager {
    pub(crate) fn set_task_termination_intent(&self, id: &str, intent: TaskTerminationIntent) {
        self.task_termination_intents
            .lock()
            .insert(id.to_string(), intent);
    }

    pub(crate) fn take_task_termination_intent(&self, id: &str) -> Option<TaskTerminationIntent> {
        self.task_termination_intents.lock().remove(id)
    }

    pub(crate) fn insert_pty_handles(
        &self,
        id: &str,
        master: Box<dyn MasterPty + Send>,
        writer: Box<dyn Write + Send>,
        child: Box<dyn Child + Send + Sync>,
    ) {
        self.insert_pty_handles_for_kind(id, ManagedPtyKind::Agent, master, writer, child);
    }

    pub(crate) fn insert_shell_pty_handles(
        &self,
        id: &str,
        master: Box<dyn MasterPty + Send>,
        writer: Box<dyn Write + Send>,
        child: Box<dyn Child + Send + Sync>,
    ) {
        self.insert_pty_handles_for_kind(id, ManagedPtyKind::Shell, master, writer, child);
    }

    fn insert_pty_handles_for_kind(
        &self,
        id: &str,
        kind: ManagedPtyKind,
        master: Box<dyn MasterPty + Send>,
        writer: Box<dyn Write + Send>,
        child: Box<dyn Child + Send + Sync>,
    ) {
        self.pty_masters
            .lock()
            .insert(id.to_string(), Arc::new(Mutex::new(master)));
        self.pty_writers
            .lock()
            .insert(id.to_string(), Arc::new(Mutex::new(writer)));
        self.child_handles
            .lock()
            .insert(id.to_string(), Arc::new(Mutex::new(child)));
        self.managed_sessions
            .lock()
            .insert(id.to_string(), ManagedPtySession::new(kind));
    }

    pub(crate) fn write_to_pty(&self, id: &str, data: &[u8], flush: bool) -> Result<(), String> {
        let writer = self.pty_writers.lock().get(id).cloned();
        let Some(writer) = writer else {
            return Err(format!("找不到任务 {id} 的活动 PTY 写入器"));
        };
        let mut writer = writer.lock();
        writer.write_all(data).map_err(|err| err.to_string())?;
        if flush {
            writer.flush().map_err(|err| err.to_string())?;
        }
        Ok(())
    }

    pub(crate) fn resize_registered_pty(&self, id: &str, size: PtySize) -> Result<(), String> {
        let master = self.pty_masters.lock().get(id).cloned();
        let Some(master) = master else {
            return Ok(());
        };
        let result = master.lock().resize(size).map_err(|err| err.to_string());
        result
    }

    pub(crate) fn try_wait_child(&self, id: &str) -> Result<Option<ExitStatus>, String> {
        let child = self.child_handles.lock().get(id).cloned();
        let Some(child) = child else {
            return Ok(None);
        };
        let mut child = child.lock();
        child.try_wait().map_err(|err| err.to_string())
    }

    pub(crate) fn kill_child(&self, id: &str) -> Result<(), String> {
        if let Some(session) = self.managed_sessions.lock().get_mut(id) {
            session.status = ManagedPtyStatus::Stopping;
        }
        let child = self.child_handles.lock().get(id).cloned();
        let Some(child) = child else {
            return Ok(());
        };
        let mut child = child.lock();
        child.kill().map_err(|err| err.to_string())
    }

    pub(crate) fn append_output(&self, id: &str, data: &str) -> Option<u64> {
        let mut sessions = self.managed_sessions.lock();
        let session = sessions.get_mut(id)?;
        Some(session.output.push(data))
    }

    pub fn output_snapshot(&self, id: &str) -> Result<ManagedPtySnapshot, String> {
        let sessions = self.managed_sessions.lock();
        let Some(session) = sessions.get(id) else {
            return Err(format!("找不到会话 {id} 的后端输出快照"));
        };
        Ok(session.output.snapshot())
    }

    pub(crate) fn mark_finished(&self, id: &str) {
        if let Some(session) = self.managed_sessions.lock().get_mut(id) {
            session.status = ManagedPtyStatus::Finished;
        }
    }

    /// Atomically remove a task or shell from all PTY maps and cancel session watchers.
    /// Locks are acquired in a fixed order to prevent deadlocks.
    pub(crate) fn remove_pty_handles(&self, id: &str) {
        let mut masters = self.pty_masters.lock();
        let mut writers = self.pty_writers.lock();
        let mut children = self.child_handles.lock();
        let mut intents = self.task_termination_intents.lock();
        let mut watchers = self.session_watchers.lock();
        let mut sessions = self.managed_sessions.lock();

        masters.remove(id);
        writers.remove(id);
        children.remove(id);
        intents.remove(id);
        if let Some(token) = watchers.remove(id) {
            token.cancel();
        }
        if let Some(session) = sessions.get_mut(id) {
            session.status = ManagedPtyStatus::Released;
        }
        if sessions
            .get(id)
            .is_some_and(|session| session.kind == ManagedPtyKind::Shell)
        {
            sessions.remove(id);
        }
    }

    pub(crate) fn shutdown_all(&self) {
        let ids = self
            .child_handles
            .lock()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for id in ids {
            let _ = self.kill_child(&id);
            self.remove_pty_handles(&id);
        }
    }
}

impl Drop for TaskManager {
    fn drop(&mut self) {
        self.shutdown_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── CancellationToken tests ──────────────────────────────────────────

    #[test]
    fn cancellation_token_starts_not_cancelled() {
        let token = CancellationToken::new();
        assert!(!token.is_cancelled());
    }

    #[test]
    fn cancellation_token_cancel_sets_flag() {
        let token = CancellationToken::new();
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn cancellation_token_clone_shares_state() {
        let token = CancellationToken::new();
        let clone = token.clone();
        assert!(!clone.is_cancelled());

        token.cancel();
        assert!(clone.is_cancelled());
        assert!(token.is_cancelled());
    }

    #[test]
    fn cancellation_token_cancel_from_clone() {
        let token = CancellationToken::new();
        let clone = token.clone();
        clone.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn cancellation_token_double_cancel_is_idempotent() {
        let token = CancellationToken::new();
        token.cancel();
        token.cancel();
        assert!(token.is_cancelled());
    }

    // ── TaskTerminationIntent tests ──────────────────────────────────────

    #[test]
    fn termination_intent_equality() {
        assert_eq!(
            TaskTerminationIntent::Stopped,
            TaskTerminationIntent::Stopped
        );
    }

    #[test]
    fn termination_intent_debug_format() {
        let intent = TaskTerminationIntent::Stopped;
        assert_eq!(format!("{:?}", intent), "Stopped");
    }

    // ── TaskManager termination intent tests ─────────────────────────────

    #[test]
    fn set_and_take_termination_intent() {
        let mgr = TaskManager::default();
        mgr.set_task_termination_intent("task-1", TaskTerminationIntent::Stopped);

        let taken = mgr.take_task_termination_intent("task-1");
        assert_eq!(taken, Some(TaskTerminationIntent::Stopped));

        // Second take returns None (removed)
        let taken2 = mgr.take_task_termination_intent("task-1");
        assert_eq!(taken2, None);
    }

    #[test]
    fn take_nonexistent_intent_returns_none() {
        let mgr = TaskManager::default();
        assert_eq!(mgr.take_task_termination_intent("no-such-task"), None);
    }

    #[test]
    fn set_overwrites_previous_intent() {
        let mgr = TaskManager::default();
        mgr.set_task_termination_intent("t1", TaskTerminationIntent::Stopped);
        mgr.set_task_termination_intent("t1", TaskTerminationIntent::Stopped);

        // Should still have exactly one entry
        assert_eq!(
            mgr.take_task_termination_intent("t1"),
            Some(TaskTerminationIntent::Stopped)
        );
        assert_eq!(mgr.take_task_termination_intent("t1"), None);
    }

    // ── TaskManager PTY write tests ──────────────────────────────────────

    #[test]
    fn write_to_missing_pty_returns_error() {
        let mgr = TaskManager::default();
        let result = mgr.write_to_pty("nonexistent", b"data", false);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("nonexistent"));
    }

    // ── TaskManager resize tests ─────────────────────────────────────────

    #[test]
    fn resize_missing_pty_returns_ok() {
        let mgr = TaskManager::default();
        let result = mgr.resize_registered_pty(
            "nonexistent",
            PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            },
        );
        assert!(result.is_ok());
    }

    // ── TaskManager child handle tests ───────────────────────────────────

    #[test]
    fn try_wait_missing_child_returns_none() {
        let mgr = TaskManager::default();
        let result = mgr.try_wait_child("nonexistent");
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn kill_missing_child_returns_ok() {
        let mgr = TaskManager::default();
        let result = mgr.kill_child("nonexistent");
        assert!(result.is_ok());
    }

    // ── TaskManager remove_pty_handles tests ─────────────────────────────

    #[test]
    fn remove_missing_handles_does_not_panic() {
        let mgr = TaskManager::default();
        mgr.remove_pty_handles("nonexistent");
        // Should complete without panic
    }

    #[test]
    fn remove_handles_cancels_session_watcher() {
        let mgr = TaskManager::default();
        let token = CancellationToken::new();
        mgr.session_watchers
            .lock()
            .insert("task-1".to_string(), token.clone());

        assert!(!token.is_cancelled());
        mgr.remove_pty_handles("task-1");
        assert!(token.is_cancelled());
    }

    #[test]
    fn remove_handles_cleans_up_all_maps() {
        let mgr = TaskManager::default();

        // Insert a termination intent
        mgr.set_task_termination_intent("task-1", TaskTerminationIntent::Stopped);
        // Insert a session watcher
        mgr.session_watchers
            .lock()
            .insert("task-1".to_string(), CancellationToken::new());

        mgr.remove_pty_handles("task-1");

        // Termination intent should be gone
        assert_eq!(mgr.take_task_termination_intent("task-1"), None);
        // Session watcher should be gone
        assert!(mgr.session_watchers.lock().get("task-1").is_none());
    }

    // ── TaskManager default construction ─────────────────────────────────

    #[test]
    fn default_task_manager_has_empty_maps() {
        let mgr = TaskManager::default();
        assert!(mgr.pty_masters.lock().is_empty());
        assert!(mgr.pty_writers.lock().is_empty());
        assert!(mgr.child_handles.lock().is_empty());
        assert!(mgr.task_termination_intents.lock().is_empty());
        assert!(mgr.codex_sessions.lock().is_empty());
        assert!(mgr.claude_sessions.lock().is_empty());
        assert!(mgr.claimed_session_paths.lock().is_empty());
        assert!(mgr.session_watchers.lock().is_empty());
    }

    // ── Session map tests ────────────────────────────────────────────────

    #[test]
    fn codex_sessions_map_insert_and_check() {
        let mgr = TaskManager::default();
        mgr.codex_sessions.lock().insert(
            "task-1".to_string(),
            CodexSessionInfo {
                session_id: "sess-123".to_string(),
                session_path: "/path/to/session".to_string(),
            },
        );
        assert!(mgr.codex_sessions.lock().contains_key("task-1"));
        assert!(!mgr.codex_sessions.lock().contains_key("task-2"));
    }

    #[test]
    fn claude_sessions_map_insert_and_check() {
        let mgr = TaskManager::default();
        mgr.claude_sessions.lock().insert(
            "task-1".to_string(),
            ClaudeSessionInfo {
                session_id: "sess-456".to_string(),
                session_path: "/path/to/claude".to_string(),
            },
        );
        assert!(mgr.claude_sessions.lock().contains_key("task-1"));
    }

    #[test]
    fn claimed_session_paths_insert_and_check() {
        let mgr = TaskManager::default();
        mgr.claimed_session_paths
            .lock()
            .insert("/some/path.jsonl".to_string());
        assert!(mgr
            .claimed_session_paths
            .lock()
            .contains("/some/path.jsonl"));
        assert!(!mgr
            .claimed_session_paths
            .lock()
            .contains("/other/path.jsonl"));
    }
}

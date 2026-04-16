use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::Arc;

use crate::task_runtime::session::{ClaudeSessionInfo, CodexSessionInfo};

#[derive(Default)]
pub struct TaskManager {
    pub(crate) pty_masters: Mutex<HashMap<String, Box<dyn portable_pty::MasterPty + Send>>>,
    pub(crate) pty_writers: Mutex<HashMap<String, Box<dyn Write + Send>>>,
    pub(crate) child_handles:
        Mutex<HashMap<String, Arc<std::sync::Mutex<Box<dyn portable_pty::Child + Send + Sync>>>>>,
    pub(crate) cancelled_tasks: Mutex<HashSet<String>>,
    pub(crate) codex_sessions: Mutex<HashMap<String, CodexSessionInfo>>,
    pub(crate) claude_sessions: Mutex<HashMap<String, ClaudeSessionInfo>>,
    pub(crate) claimed_session_paths: Mutex<HashSet<String>>,
    /// Maps task_id -> dispatch_id for dispatcher-spawned subprocess tracking.
    pub(crate) dispatcher_subprocess_ids: Mutex<HashMap<String, String>>,
    /// Maps task_id -> project_path for dispatcher debug logging on subprocess interactions.
    pub(crate) task_project_paths: Mutex<HashMap<String, String>>,
    /// Subprocess task_ids exited by the dispatcher via /exit (skip result injection).
    pub(crate) dispatcher_exited_subprocesses: Mutex<HashSet<String>>,
    /// Maps task_id -> AtomicBool, used by session JSONL watcher to force idle emission.
    pub(crate) dispatcher_force_idle_flags:
        Mutex<HashMap<String, std::sync::Arc<std::sync::atomic::AtomicBool>>>,
}

impl TaskManager {
    /// Atomically remove a task or shell from all PTY maps.
    /// Locks are acquired in a fixed order to prevent deadlocks.
    pub(crate) fn remove_pty_handles(&self, id: &str) {
        let mut masters = self.pty_masters.lock();
        let mut writers = self.pty_writers.lock();
        let mut children = self.child_handles.lock();
        let mut project_paths = self.task_project_paths.lock();

        masters.remove(id);
        writers.remove(id);
        children.remove(id);
        project_paths.remove(id);
    }
}

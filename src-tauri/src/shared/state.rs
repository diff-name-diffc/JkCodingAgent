use parking_lot::Mutex;
use portable_pty::{Child, ExitStatus, MasterPty, PtySize};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::Arc;

use crate::task_runtime::session::{ClaudeSessionInfo, CodexSessionInfo};

pub(crate) type SharedPtyMaster = Arc<Mutex<Box<dyn MasterPty + Send>>>;
pub(crate) type SharedPtyWriter = Arc<Mutex<Box<dyn Write + Send>>>;
pub(crate) type SharedChildHandle = Arc<Mutex<Box<dyn Child + Send + Sync>>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TaskTerminationIntent {
    Cancelled,
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
        self.pty_masters
            .lock()
            .insert(id.to_string(), Arc::new(Mutex::new(master)));
        self.pty_writers
            .lock()
            .insert(id.to_string(), Arc::new(Mutex::new(writer)));
        self.child_handles
            .lock()
            .insert(id.to_string(), Arc::new(Mutex::new(child)));
    }

    pub(crate) fn write_to_pty(&self, id: &str, data: &[u8], flush: bool) -> Result<(), String> {
        let writer = self.pty_writers.lock().get(id).cloned();
        let Some(writer) = writer else {
            return Ok(());
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
        let child = self.child_handles.lock().get(id).cloned();
        let Some(child) = child else {
            return Ok(());
        };
        let mut child = child.lock();
        child.kill().map_err(|err| err.to_string())
    }

    /// Atomically remove a task or shell from all PTY maps.
    /// Locks are acquired in a fixed order to prevent deadlocks.
    pub(crate) fn remove_pty_handles(&self, id: &str) {
        let mut masters = self.pty_masters.lock();
        let mut writers = self.pty_writers.lock();
        let mut children = self.child_handles.lock();
        let mut project_paths = self.task_project_paths.lock();
        let mut intents = self.task_termination_intents.lock();

        masters.remove(id);
        writers.remove(id);
        children.remove(id);
        project_paths.remove(id);
        intents.remove(id);
    }
}

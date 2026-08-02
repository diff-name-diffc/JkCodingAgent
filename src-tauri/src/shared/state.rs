use parking_lot::Mutex;
use portable_pty::{Child, MasterPty, PtySize};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::sync::Arc;

pub(crate) type SharedPtyMaster = Arc<Mutex<Box<dyn MasterPty + Send>>>;
pub(crate) type SharedPtyWriter = Arc<Mutex<Box<dyn Write + Send>>>;
pub(crate) type SharedChildHandle = Arc<Mutex<Box<dyn Child + Send + Sync>>>;

const SESSION_OUTPUT_MAX_BYTES: usize = 10 * 1024 * 1024;
const SESSION_OUTPUT_MAX_CHUNKS: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManagedPtyKind {
    Shell,
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
    output: SessionOutputBuffer,
}

impl ManagedPtySession {
    fn new(kind: ManagedPtyKind) -> Self {
        Self {
            kind,
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

#[derive(Default)]
pub struct TaskManager {
    pub(crate) pty_masters: Mutex<HashMap<String, SharedPtyMaster>>,
    pub(crate) pty_writers: Mutex<HashMap<String, SharedPtyWriter>>,
    pub(crate) child_handles: Mutex<HashMap<String, SharedChildHandle>>,
    managed_sessions: Mutex<HashMap<String, ManagedPtySession>>,
}

impl TaskManager {
    pub(crate) fn insert_shell_pty_handles(
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
        self.managed_sessions
            .lock()
            .insert(id.to_string(), ManagedPtySession::new(ManagedPtyKind::Shell));
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

    pub(crate) fn kill_child(&self, id: &str) -> Result<(), String> {
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

    /// Atomically remove a task or shell from all PTY maps.
    pub(crate) fn remove_pty_handles(&self, id: &str) {
        let mut masters = self.pty_masters.lock();
        let mut writers = self.pty_writers.lock();
        let mut children = self.child_handles.lock();
        let mut sessions = self.managed_sessions.lock();

        masters.remove(id);
        writers.remove(id);
        children.remove(id);
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

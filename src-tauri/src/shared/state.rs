use parking_lot::Mutex;
use portable_pty::{Child, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

pub(crate) type SharedPtyMaster = Arc<Mutex<Box<dyn MasterPty + Send>>>;
pub(crate) type SharedPtyWriter = Arc<Mutex<Box<dyn Write + Send>>>;
pub(crate) type SharedChildHandle = Arc<Mutex<Box<dyn Child + Send + Sync>>>;

#[derive(Default)]
pub struct TaskManager {
    pub(crate) pty_masters: Mutex<HashMap<String, SharedPtyMaster>>,
    pub(crate) pty_writers: Mutex<HashMap<String, SharedPtyWriter>>,
    pub(crate) child_handles: Mutex<HashMap<String, SharedChildHandle>>,
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

    /// Atomically remove a task or shell from all PTY maps.
    pub(crate) fn remove_pty_handles(&self, id: &str) {
        let mut masters = self.pty_masters.lock();
        let mut writers = self.pty_writers.lock();
        let mut children = self.child_handles.lock();

        masters.remove(id);
        writers.remove(id);
        children.remove(id);
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

use parking_lot::Mutex;
use std::collections::HashMap;
use tokio::sync::watch;

/// 每个 workspace 的运行状态。
///
/// 设计要点：
/// - **禁止重入**：同一 workspace 已有运行中的 run 时，begin 会直接报错，
///   避免并发修改同一会话状态。
/// - **取消信号**：stop 通过 watch channel 通知运行中的 run，run 内部轮询
///   cancel_rx 决定是否优雅中止。
pub(super) struct ActiveRunStore {
    entries: Mutex<HashMap<String, ActiveRunEntry>>,
}

#[derive(Clone)]
struct ActiveRunEntry {
    stop_tx: watch::Sender<bool>,
}

pub(crate) struct ActiveRunHandle {
    pub(crate) cancel_rx: watch::Receiver<bool>,
}

impl Default for ActiveRunStore {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
}

impl ActiveRunStore {
    /// 开始一次 run：禁止同 workspace 重入，并保存取消信号。
    pub(super) fn begin(&self, workspace_id: &str) -> std::result::Result<ActiveRunHandle, String> {
        let mut active_runs = self.entries.lock();
        if active_runs.contains_key(workspace_id) {
            return Err(format!(
                "会话 {} 已在运行中，请等待当前任务完成",
                workspace_id
            ));
        }
        let (stop_tx, cancel_rx) = watch::channel(false);
        active_runs.insert(workspace_id.to_string(), ActiveRunEntry { stop_tx });
        Ok(ActiveRunHandle { cancel_rx })
    }

    pub(super) fn finish(&self, workspace_id: &str) {
        self.entries.lock().remove(workspace_id);
    }

    /// 请求取消：向 watch channel 发送 true，运行中的 run 轮询到后优雅中止。
    pub(super) fn stop(&self, workspace_id: &str) -> bool {
        let tx = self
            .entries
            .lock()
            .get(workspace_id)
            .map(|entry| entry.stop_tx.clone());

        tx.is_some_and(|sender| sender.send(true).is_ok())
    }
}

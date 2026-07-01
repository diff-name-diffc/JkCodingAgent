use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::watch;

/// 每个 workspace 的运行并发控制。
///
/// 设计要点：
/// - **禁止重入**：同一 workspace 已有运行中的 run 时，begin 会直接报错，
///   避免并发修改同一会话状态。
/// - **代际校验**：finish 时校验 generation 是否匹配，防止一个旧 run 误把
///   新 run 的 entry 清掉（竞态保护）。
/// - **取消信号**：stop 通过 watch channel 通知运行中的 run，run 内部轮询
///   cancel_rx 决定是否优雅中止。
pub(super) struct ActiveRunStore {
    entries: Mutex<HashMap<String, ActiveRunEntry>>,
    next_generation: AtomicU64,
}

#[derive(Clone)]
struct ActiveRunEntry {
    generation: u64,
    stop_tx: watch::Sender<bool>,
}

pub(crate) struct ActiveRunHandle {
    pub(crate) generation: u64,
    pub(crate) cancel_rx: watch::Receiver<bool>,
}

impl Default for ActiveRunStore {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            next_generation: AtomicU64::new(1),
        }
    }
}

impl ActiveRunStore {
    /// 开始一次 run：禁止重入，并为这次 run 分配唯一 generation + 取消信号通道。
    pub(super) fn begin(&self, workspace_id: &str) -> std::result::Result<ActiveRunHandle, String> {
        let mut active_runs = self.entries.lock();
        if active_runs.contains_key(workspace_id) {
            return Err(format!(
                "会话 {} 已在运行中，请等待当前任务完成",
                workspace_id
            ));
        }
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let (stop_tx, cancel_rx) = watch::channel(false);
        active_runs.insert(
            workspace_id.to_string(),
            ActiveRunEntry {
                generation,
                stop_tx,
            },
        );
        Ok(ActiveRunHandle {
            generation,
            cancel_rx,
        })
    }

    pub(super) fn finish(&self, workspace_id: &str, generation: u64) {
        let mut active_runs = self.entries.lock();
        // 仅当代际匹配才移除 entry——防止一个旧 run 的 finish 误清掉新 run，
        // 这在异步并发场景下是关键的安全保障。
        let should_remove = active_runs
            .get(workspace_id)
            .is_some_and(|entry| entry.generation == generation);
        if should_remove {
            active_runs.remove(workspace_id);
        }
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

use parking_lot::Mutex;
use std::collections::HashMap;
use tokio::sync::{mpsc, watch};

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

/// 图运行注册表：同一 plan 禁止重入；cancel 通过 watch 通知运行器。
///
/// 与 `ActiveRunStore` 同构但按 plan_id 索引——图执行独立于会话 run
/// （用户在图运行期间仍可与会话对话）。
///
/// v3 增加 resume 通道（mpsc，带缓冲）：高危写检查点暂停后，`graph_run_resume`
/// 命令通过它唤醒运行器。取消信号保持 watch<bool> 不变，node_exec 的取消轮询零改动。
struct GraphRunEntry {
    cancel_tx: watch::Sender<bool>,
    resume_tx: mpsc::UnboundedSender<()>,
}

/// 运行器持有的图运行句柄：取消信号 + 恢复信号接收端。
pub(crate) struct GraphRunHandle {
    pub(crate) cancel_rx: watch::Receiver<bool>,
    pub(crate) resume_rx: mpsc::UnboundedReceiver<()>,
}

pub(super) struct GraphRunRegistry {
    entries: Mutex<HashMap<String, GraphRunEntry>>,
}

impl Default for GraphRunRegistry {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
}

impl GraphRunRegistry {
    pub(super) fn begin(&self, plan_id: &str) -> std::result::Result<GraphRunHandle, String> {
        let mut entries = self.entries.lock();
        if entries.contains_key(plan_id) {
            return Err("该图正在运行中，请勿重复启动".to_string());
        }
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (resume_tx, resume_rx) = mpsc::unbounded_channel();
        entries.insert(
            plan_id.to_string(),
            GraphRunEntry { cancel_tx, resume_tx },
        );
        Ok(GraphRunHandle { cancel_rx, resume_rx })
    }

    pub(super) fn finish(&self, plan_id: &str) {
        self.entries.lock().remove(plan_id);
    }

    /// 请求取消：向 watch channel 发送 true，图运行器轮询到后执行取消语义。
    pub(super) fn cancel(&self, plan_id: &str) -> bool {
        self.entries
            .lock()
            .get(plan_id)
            .is_some_and(|entry| entry.cancel_tx.send(true).is_ok())
    }

    /// 恢复暂停中的图运行（高危写检查点）。mpsc 带缓冲：检查点等待前到达的
    /// resume 不会丢失。
    pub(super) fn resume(&self, plan_id: &str) -> bool {
        self.entries
            .lock()
            .get(plan_id)
            .is_some_and(|entry| entry.resume_tx.send(()).is_ok())
    }
}

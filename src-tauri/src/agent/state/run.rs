use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, watch};

/// 每个 workspace 的运行状态。
///
/// 设计要点：
/// - **禁止重入**：同一 workspace 已有运行中的 run 时，begin 会直接报错，
///   避免并发修改同一会话状态。
/// - **取消信号**：stop 通过 watch channel 通知运行中的 run，run 内部轮询
///   cancel_rx 决定是否优雅中止。
/// - **RAII 清理（G11-09）**：begin 返回的句柄持有代际校验的清理守卫，
///   入口与 worker 共持同代租约，最后一个句柄丢弃时才移除注册条目，
///   不再依赖"必定执行到 finish"的调用纪律。
/// - **代际 token（G11-10）**：条目携带递增 epoch，清理仅移除与自己同代的
///   条目——迟到的旧 run 结算不会误删新 run 的注册。
pub(super) struct ActiveRunStore {
    data: Arc<Mutex<ActiveRunData>>,
}

struct ActiveRunData {
    entries: HashMap<String, ActiveRunEntry>,
    next_epoch: u64,
}

struct ActiveRunEntry {
    stop_tx: watch::Sender<bool>,
    epoch: u64,
}

/// 运行中的 run 句柄：取消信号接收端 + RAII 清理守卫。
///
/// 注意：字段不可移出（守卫依赖完整 drop），需要 cancel_rx 请用
/// `cancel_receiver()` 克隆。
#[derive(Clone)]
pub(crate) struct ActiveRunHandle {
    cancel_rx: watch::Receiver<bool>,
    /// RAII 清理守卫：Drop 时移除注册条目；`request_cancel` 亦经它按
    /// (workspace_id, epoch) 定位本代取消发送端。
    cleanup: Arc<ActiveRunCleanup>,
}

tokio::task_local! { static RUN_LEASE: ActiveRunHandle; }

impl ActiveRunHandle {
    pub(crate) fn request_cancel(&self) {
        let sender = {
            let data = self.cleanup.data.lock();
            data.entries
                .get(&self.cleanup.key)
                .filter(|entry| entry.epoch == self.cleanup.epoch)
                .map(|entry| entry.stop_tx.clone())
        };
        if let Some(sender) = sender {
            sender.send_replace(true);
        }
    }
    pub(crate) fn current() -> Option<Self> {
        RUN_LEASE.try_with(Clone::clone).ok()
    }
    pub(crate) async fn scope<F: std::future::Future>(&self, future: F) -> F::Output {
        RUN_LEASE.scope(self.clone(), future).await
    }

    /// 取消信号接收端的克隆（watch 广播语义，原句柄保留供 RAII 清理）。
    pub(crate) fn cancel_receiver(&self) -> watch::Receiver<bool> {
        self.cancel_rx.clone()
    }
}

/// Drop 时按 (workspace_id, epoch) 双重校验移除注册条目。
struct ActiveRunCleanup {
    data: Arc<Mutex<ActiveRunData>>,
    key: String,
    epoch: u64,
}

impl Drop for ActiveRunCleanup {
    fn drop(&mut self) {
        let mut data = self.data.lock();
        // 仅当条目仍属于本代时才移除：若已有更新的 run begin 并登记，
        // 迟到的清理不得误删新条目（G11-10 的代际防护）。
        if data
            .entries
            .get(&self.key)
            .is_some_and(|entry| entry.epoch == self.epoch)
        {
            data.entries.remove(&self.key);
        }
    }
}

impl Default for ActiveRunStore {
    fn default() -> Self {
        Self {
            data: Arc::new(Mutex::new(ActiveRunData {
                entries: HashMap::new(),
                next_epoch: 1,
            })),
        }
    }
}

impl ActiveRunStore {
    /// 开始一次 run：禁止同 workspace 重入，并保存取消信号。
    /// epoch 分配与条目写入在同一把锁内完成（原子登记）。
    pub(super) fn begin(&self, workspace_id: &str) -> std::result::Result<ActiveRunHandle, String> {
        let mut data = self.data.lock();
        // 兜底清理（G11-09）：取消接收端全部消失说明运行方已不复存在
        // （句柄异常丢失等极端路径），残留条目在此回收，避免永久卡死重入。
        data.entries
            .retain(|_, entry| entry.stop_tx.receiver_count() > 0);
        if data.entries.contains_key(workspace_id) {
            return Err(format!(
                "会话 {} 已在运行中，请等待当前任务完成",
                workspace_id
            ));
        }
        let epoch = data.next_epoch;
        data.next_epoch = data.next_epoch.wrapping_add(1);
        let (stop_tx, cancel_rx) = watch::channel(false);
        data.entries
            .insert(workspace_id.to_string(), ActiveRunEntry { stop_tx, epoch });
        Ok(ActiveRunHandle {
            cancel_rx,
            cleanup: Arc::new(ActiveRunCleanup {
                data: Arc::clone(&self.data),
                key: workspace_id.to_string(),
                epoch,
            }),
        })
    }

    /// 显式结束（G11-10：消费句柄触发代际校验清理）。与句柄自然 drop 等价，
    /// 保留此入口是为了让调用方的"运行结束"意图显式可读。
    pub(super) fn finish(&self, handle: ActiveRunHandle) {
        drop(handle);
    }

    /// 会话是否有运行中的 run。fail-closed 守卫（删除/清空/截断命令拒绝
    /// 运行中会话）与前端重载对账共用。读取前套用与 begin 相同的兜底回收，
    /// 避免极端路径残留的死条目（接收端归零）让查询误报运行中。
    pub(super) fn is_running(&self, workspace_id: &str) -> bool {
        let mut data = self.data.lock();
        data.entries
            .retain(|_, entry| entry.stop_tx.receiver_count() > 0);
        data.entries.contains_key(workspace_id)
    }

    /// 当前全部有运行中 run 的 workspace id（无序）。
    pub(super) fn active_run_ids(&self) -> Vec<String> {
        let mut data = self.data.lock();
        data.entries
            .retain(|_, entry| entry.stop_tx.receiver_count() > 0);
        data.entries.keys().cloned().collect()
    }

    /// 请求取消当前 run：向 watch channel 发送 true，运行中的 run 轮询到后优雅中止。
    /// 按 workspace 定位"当前"条目是 UI 停止按钮的语义（无需调用方持有代际）。
    pub(super) fn stop(&self, workspace_id: &str) -> bool {
        let tx = self
            .data
            .lock()
            .entries
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
/// 生命周期清理（G11-09）：图运行句柄的 cancel/resume 接收端会被运行器
/// （graph/runner.rs）按字段消费，句柄本体无法实现 Drop 清理（部分移入
/// 类型的剩余字段不会被 drop），因此这里用「begin 时按接收端存活回收」
/// 达成等价的无残留保证：运行器消失（panic/abort/正常结束）后接收端归零，
/// 下一次 begin 回收槽位，同一 plan 可再次启动。
///
/// resume 通道（G11-12）：容量 1 的有界 mpsc + try_send 去重——
/// 「检查点暂停前到达的 resume 不丢失」仍满足，但陈旧/重复信号不再堆积，
/// 无法让后续新的确认暂停被旧信号直接跳过。
struct GraphRunEntry {
    cancel_tx: watch::Sender<bool>,
    resume_tx: mpsc::Sender<()>,
}

/// 运行器持有的图运行句柄：取消信号 + 恢复信号接收端。
pub(crate) struct GraphRunHandle {
    pub(crate) cancel_rx: watch::Receiver<bool>,
    pub(crate) resume_rx: mpsc::Receiver<()>,
}

pub(super) struct GraphRunRegistry {
    data: Mutex<GraphRegistryData>,
}

struct GraphRegistryData {
    entries: HashMap<String, GraphRunEntry>,
}

impl Default for GraphRunRegistry {
    fn default() -> Self {
        Self {
            data: Mutex::new(GraphRegistryData {
                entries: HashMap::new(),
            }),
        }
    }
}

impl GraphRunRegistry {
    pub(super) fn begin(&self, plan_id: &str) -> std::result::Result<GraphRunHandle, String> {
        let mut data = self.data.lock();
        // 兜底清理（G11-09）：取消接收端归零说明运行器已消失——panic/abort/
        // 正常结束但 finish 未到——残留条目在此回收，槽位不会永久卡死。
        data.entries
            .retain(|_, entry| entry.cancel_tx.receiver_count() > 0);
        if data.entries.contains_key(plan_id) {
            return Err("该图正在运行中，请勿重复启动".to_string());
        }
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (resume_tx, resume_rx) = mpsc::channel(1);
        data.entries.insert(
            plan_id.to_string(),
            GraphRunEntry {
                cancel_tx,
                resume_tx,
            },
        );
        Ok(GraphRunHandle {
            cancel_rx,
            resume_rx,
        })
    }

    /// 结束登记。仅当条目的取消接收端已全部消失（运行器确已退出）时才移除——
    /// 防止旧 run 迟到的 finish 误删新 run 的条目（G11-10 代际防护的等价形式：
    /// 新 run 的条目必然持有存活的接收端）。接收端仍在的条目留给下一次 begin
    /// 的兜底回收（如启动早期失败路径：置 running 失败时句柄尚未进入运行器）。
    pub(super) fn finish(&self, plan_id: &str) {
        let mut data = self.data.lock();
        let removable = data
            .entries
            .get(plan_id)
            .is_some_and(|entry| entry.cancel_tx.receiver_count() == 0);
        if removable {
            data.entries.remove(plan_id);
        }
    }

    /// 请求取消：向 watch channel 发送 true，图运行器轮询到后执行取消语义。
    pub(super) fn cancel(&self, plan_id: &str) -> bool {
        self.data
            .lock()
            .entries
            .get(plan_id)
            .is_some_and(|entry| entry.cancel_tx.send(true).is_ok())
    }

    /// 恢复暂停中的图运行（高危写检查点）。
    ///
    /// G11-11：恢复前先复查取消状态——cancel 已置位时拒绝 resume，
    /// 避免「cancel 就绪但 resume 后到」时陈旧恢复信号放行已取消的运行。
    /// G11-12：容量 1 有界通道 + try_send：已有恢复信号缓冲时去重返回 false，
    /// 杜绝无界缓冲下陈旧信号跳过后续确认暂停。
    pub(super) fn resume(&self, plan_id: &str) -> bool {
        let data = self.data.lock();
        let Some(entry) = data.entries.get(plan_id) else {
            return false;
        };
        if *entry.cancel_tx.borrow() {
            return false;
        }
        entry.resume_tx.try_send(()).is_ok()
    }
}

/// 架构画布程序执行注册表：architecture_run 工具与前端画布执行器之间的
/// 一次性请求/响应桥。
///
/// 工具侧 `begin` 登记 run_id → oneshot 接收端并 emit 事件；前端执行完画布
/// 程序后经 `architecture_run_complete` 命令调 `complete` 解除等待。工具侧
/// 超时/取消路径必须调 `remove` 清槽——此后迟到的 `complete` 找不到条目返回
/// false，无副作用（天然幂等）。
///
/// 条目同时记录发起会话的 workspace_id：`complete` 校验回传方与登记方属于
/// 同一会话（与 `dispatcher_get_tool_artifact` 等命令的 workspace 域校验
/// 风格对齐），错会话回传按未消费处理、槽位保留给真正的主人。
pub(super) struct ArchRunRegistry {
    entries: Mutex<HashMap<String, ArchRunEntry>>,
}

struct ArchRunEntry {
    started: bool,
    sender: oneshot::Sender<String>,
    workspace_id: String,
}

impl Default for ArchRunRegistry {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
}

impl ArchRunRegistry {
    /// 登记一次画布程序执行，返回 (run_id, 报告接收端)。
    pub(super) fn begin(&self, workspace_id: &str) -> (String, oneshot::Receiver<String>) {
        let run_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        let mut entries = self.entries.lock();
        // 兜底回收：工具 future 被整体丢弃（abort/panic）时无人调 remove，
        // 借登记之机清掉接收端已关闭的死条目（与 GraphRunRegistry 同思路）。
        entries.retain(|_, entry| !entry.sender.is_closed());
        entries.insert(
            run_id.clone(),
            ArchRunEntry {
                started: false,
                sender: tx,
                workspace_id: workspace_id.to_string(),
            },
        );
        (run_id, rx)
    }

    /// 前端提交草稿前取得唯一执行权；取消先发生时禁止迟到执行。
    pub(super) fn claim(&self, run: &str, workspace: &str) -> bool {
        let mut entries = self.entries.lock();
        let Some(entry) = entries.get_mut(run) else {
            return false;
        };
        if entry.workspace_id != workspace || entry.started {
            return false;
        }
        entry.started = true;
        true
    }

    /// 只允许取消尚未取得执行权的程序；已启动程序必须等前端结算确认。
    pub(super) fn cancel_unstarted(&self, run: &str) -> bool {
        let mut entries = self.entries.lock();
        if entries.get(run).is_some_and(|entry| !entry.started) {
            entries.remove(run);
            true
        } else {
            false
        }
    }

    /// 前端回传执行报告：取出并解除等待。条目不存在（超时已清槽/重复回传）、
    /// workspace 不匹配或接收端已关闭（工具侧提前退出）时返回 false。
    pub(super) fn complete(&self, run_id: &str, workspace_id: &str, report: String) -> bool {
        // 校验与取出在锁作用域内完成，send 留到锁外——oneshot::send 虽为
        // 非阻塞实现，持锁期间调用外部类型仍违背「持锁禁止 I/O/阻塞」约定。
        let entry = {
            let mut entries = self.entries.lock();
            let matches_scope = entries
                .get(run_id)
                .is_some_and(|entry| entry.workspace_id == workspace_id);
            if !matches_scope {
                return false;
            }
            entries.remove(run_id)
        };
        entry.is_some_and(|entry| entry.sender.send(report).is_ok())
    }

    /// 工具侧超时/取消路径的显式清槽，防止条目泄漏。
    pub(super) fn remove(&self, run_id: &str) {
        self.entries.lock().remove(run_id);
    }
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;

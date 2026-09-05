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
///   panic / abort / 提前 return 等任何路径丢弃句柄都会移除注册条目，
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
pub(crate) struct ActiveRunHandle {
    cancel_rx: watch::Receiver<bool>,
    /// RAII 清理守卫：用途在其 Drop 副作用（移除注册条目），字段值本身不被读取。
    #[allow(dead_code)]
    cleanup: ActiveRunCleanup,
}

impl ActiveRunHandle {
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
            cleanup: ActiveRunCleanup {
                data: Arc::clone(&self.data),
                key: workspace_id.to_string(),
                epoch,
            },
        })
    }

    /// 显式结束（G11-10：消费句柄触发代际校验清理）。与句柄自然 drop 等价，
    /// 保留此入口是为了让调用方的"运行结束"意图显式可读。
    pub(super) fn finish(&self, handle: ActiveRunHandle) {
        drop(handle);
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
    /// 本次运行的代际号（begin 单调递增），供调用方做代际校验与诊断。
    #[allow(dead_code)]
    pub(crate) epoch: u64,
}

pub(super) struct GraphRunRegistry {
    data: Mutex<GraphRegistryData>,
}

struct GraphRegistryData {
    entries: HashMap<String, GraphRunEntry>,
    next_epoch: u64,
}

impl Default for GraphRunRegistry {
    fn default() -> Self {
        Self {
            data: Mutex::new(GraphRegistryData {
                entries: HashMap::new(),
                next_epoch: 1,
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
        let epoch = data.next_epoch;
        data.next_epoch = data.next_epoch.wrapping_add(1);
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
            epoch,
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
pub(super) struct ArchRunRegistry {
    entries: Mutex<HashMap<String, oneshot::Sender<String>>>,
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
    pub(super) fn begin(&self) -> (String, oneshot::Receiver<String>) {
        let run_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        let mut entries = self.entries.lock();
        // 兜底回收：工具 future 被整体丢弃（abort/panic）时无人调 remove，
        // 借登记之机清掉接收端已关闭的死条目（与 GraphRunRegistry 同思路）。
        entries.retain(|_, sender| !sender.is_closed());
        entries.insert(run_id.clone(), tx);
        (run_id, rx)
    }

    /// 前端回传执行报告：取出并解除等待。条目不存在（超时已清槽/重复回传）
    /// 或接收端已关闭（工具侧提前退出）时返回 false。
    pub(super) fn complete(&self, run_id: &str, report: String) -> bool {
        let sender = self.entries.lock().remove(run_id);
        sender.is_some_and(|tx| tx.send(report).is_ok())
    }

    /// 工具侧超时/取消路径的显式清槽，防止条目泄漏。
    pub(super) fn remove(&self, run_id: &str) {
        self.entries.lock().remove(run_id);
    }
}

#[cfg(test)]
mod tests {
    use super::{ActiveRunCleanup, ActiveRunStore, ArchRunRegistry, GraphRunRegistry};
    use std::sync::Arc;

    #[test]
    fn active_run_reentry_is_rejected_until_handle_drops() {
        let store = ActiveRunStore::default();
        let handle = store.begin("ws-1").unwrap();
        assert!(store.begin("ws-1").is_err());

        drop(handle); // 模拟 panic/提前 return 路径：RAII 清理
        assert!(store.begin("ws-1").is_ok());
    }

    #[test]
    fn stale_generation_cleanup_does_not_remove_new_entry() {
        let store = ActiveRunStore::default();
        let first = store.begin("ws-1").unwrap();
        let first_epoch = first.cleanup.epoch;
        drop(first); // 正常结算，条目移除

        let second = store.begin("ws-1").unwrap();
        // 模拟旧代迟到的清理在新代登记后才到达：不得误删新代条目（G11-10）。
        let stale_cleanup = ActiveRunCleanup {
            data: Arc::clone(&store.data),
            key: "ws-1".to_string(),
            epoch: first_epoch,
        };
        drop(stale_cleanup);

        assert!(store.stop("ws-1")); // 新代条目仍在
        drop(second);
        assert!(!store.stop("ws-1")); // 新代句柄 drop 后才移除
    }

    #[test]
    fn stop_targets_current_run_and_finish_is_idempotent() {
        let store = ActiveRunStore::default();
        assert!(!store.stop("ws-1"));
        let handle = store.begin("ws-1").unwrap();
        assert!(store.stop("ws-1"));
        assert!(*handle.cancel_receiver().borrow());
        store.finish(handle);
        assert!(!store.stop("ws-1"));
        // 重复 finish 场景：再次 begin/finish 不受影响。
        let again = store.begin("ws-1").unwrap();
        store.finish(again);
        assert!(store.begin("ws-1").is_ok());
    }

    #[test]
    fn graph_run_reentry_rejected_and_finish_requires_dead_receivers() {
        let registry = GraphRunRegistry::default();
        let handle = registry.begin("plan-1").unwrap();
        assert!(registry.begin("plan-1").is_err());

        // 接收端存活时 finish 不得移除条目（保护新 run 不被旧 finish 误删）。
        registry.finish("plan-1");
        assert!(registry.begin("plan-1").is_err());

        drop(handle.resume_rx);
        drop(handle.cancel_rx);
        // 接收端归零后 finish 才真正移除。
        registry.finish("plan-1");
        assert!(registry.begin("plan-1").is_ok());
    }

    #[test]
    fn graph_run_slot_reclaimed_after_receivers_gone() {
        // G11-09 兜底：finish 未到达（句柄丢失/panic 路径）时，
        // 接收端归零后下一次 begin 回收槽位，同一 plan 可再次启动。
        let registry = GraphRunRegistry::default();
        let handle = registry.begin("plan-1").unwrap();
        drop(handle.resume_rx);
        drop(handle.cancel_rx);
        // 不调用 finish——直接 begin 触发兜底回收。
        assert!(registry.begin("plan-1").is_ok());
    }

    #[tokio::test]
    async fn resume_is_deduplicated_and_refused_after_cancel() {
        let registry = GraphRunRegistry::default();
        let mut handle = registry.begin("plan-1").unwrap();

        // 第一个 resume 缓冲成功（暂停前到达不丢失）；重复信号去重。
        assert!(registry.resume("plan-1"));
        assert!(!registry.resume("plan-1"));
        assert!(handle.resume_rx.try_recv().is_ok());
        assert!(handle.resume_rx.try_recv().is_err());

        // cancel 之后 resume 被拒绝（恢复前复查取消状态）。
        assert!(registry.cancel("plan-1"));
        assert!(!registry.resume("plan-1"));
    }

    #[tokio::test]
    async fn arch_run_complete_delivers_report_once() {
        let registry = ArchRunRegistry::default();
        let (run_id, rx) = registry.begin();
        assert!(registry.complete(&run_id, "画布程序执行成功".to_string()));
        assert_eq!(rx.await.unwrap(), "画布程序执行成功");
        // 重复回传：条目已消费，返回 false 无副作用。
        assert!(!registry.complete(&run_id, "重复报告".to_string()));
    }

    #[tokio::test]
    async fn arch_run_remove_makes_late_complete_noop() {
        let registry = ArchRunRegistry::default();
        let (run_id, rx) = registry.begin();
        registry.remove(&run_id); // 工具侧超时清槽
        assert!(!registry.complete(&run_id, "迟到的报告".to_string()));
        // 发送端被移除 → 接收端收到 RecvError，工具侧按可恢复错误处理。
        assert!(rx.await.is_err());
    }

    #[tokio::test]
    async fn arch_run_receiver_dropped_yields_failed_send() {
        let registry = ArchRunRegistry::default();
        let (run_id, rx) = registry.begin();
        drop(rx); // 工具侧提前退出（取消/超时后 rx 被丢弃前未清槽的极端路径）
        assert!(!registry.complete(&run_id, "报告".to_string()));
    }
}

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
fn worker_lease_keeps_guard_and_cancellation_after_entry_finishes() {
    let store = ActiveRunStore::default();
    let entry = store.begin("ws-1").unwrap();
    let worker = entry.clone();
    store.finish(entry);
    assert!(store.is_running("ws-1"));
    assert!(store.begin("ws-1").is_err());
    assert!(store.stop("ws-1"));
    assert!(*worker.cancel_receiver().borrow());
    drop(worker);
    assert!(!store.is_running("ws-1"));
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
fn is_running_and_active_run_ids_track_entries_per_workspace() {
    let store = ActiveRunStore::default();
    assert!(!store.is_running("ws-1"));
    assert!(store.active_run_ids().is_empty());

    let handle = store.begin("ws-1").unwrap();
    assert!(store.is_running("ws-1"));
    assert!(!store.is_running("ws-2"));
    // 按 workspace 分键：ws-1 运行不阻塞 ws-2 查询/注册。
    let other = store.begin("ws-2").unwrap();
    let mut ids = store.active_run_ids();
    ids.sort();
    assert_eq!(ids, vec!["ws-1".to_string(), "ws-2".to_string()]);

    drop(other);
    assert!(!store.is_running("ws-2"));
    let mut ids = store.active_run_ids();
    ids.sort();
    assert_eq!(ids, vec!["ws-1".to_string()]);
    drop(handle);
    assert!(store.active_run_ids().is_empty());
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
    let (run_id, rx) = registry.begin("ws-1");
    assert!(registry.complete(&run_id, "ws-1", "画布程序执行成功".to_string()));
    assert_eq!(rx.await.unwrap(), "画布程序执行成功");
    // 重复回传：条目已消费，返回 false 无副作用。
    assert!(!registry.complete(&run_id, "ws-1", "重复报告".to_string()));
}

#[tokio::test]
async fn arch_run_complete_rejects_foreign_workspace() {
    let registry = ArchRunRegistry::default();
    let (run_id, rx) = registry.begin("ws-owner");
    // 错会话回传：按未消费处理，槽位保留。
    assert!(!registry.complete(&run_id, "ws-other", "越权报告".to_string()));
    assert!(registry.complete(&run_id, "ws-owner", "正确会话报告".to_string()));
    assert_eq!(rx.await.unwrap(), "正确会话报告");
}

#[tokio::test]
async fn arch_run_remove_makes_late_complete_noop() {
    let registry = ArchRunRegistry::default();
    let (run_id, rx) = registry.begin("ws-1");
    registry.remove(&run_id); // 工具侧超时清槽
    assert!(!registry.complete(&run_id, "ws-1", "迟到的报告".to_string()));
    // 发送端被移除 → 接收端收到 RecvError，工具侧按可恢复错误处理。
    assert!(rx.await.is_err());
}

#[tokio::test]
async fn arch_run_receiver_dropped_yields_failed_send() {
    let registry = ArchRunRegistry::default();
    let (run_id, rx) = registry.begin("ws-1");
    drop(rx); // 工具侧提前退出（取消/超时后 rx 被丢弃前未清槽的极端路径）
    assert!(!registry.complete(&run_id, "ws-1", "报告".to_string()));
}

#[tokio::test]
async fn canvas_cancel_before_claim_prevents_late_execution() {
    let registry = ArchRunRegistry::default();
    let (id, rx) = registry.begin("workspace");
    assert!(registry.cancel_unstarted(&id));
    assert!(!registry.claim(&id, "workspace"));
    assert!(rx.await.is_err());
}

#[tokio::test]
async fn canvas_cancel_after_claim_keeps_report_channel_until_settled() {
    let registry = ArchRunRegistry::default();
    let (id, rx) = registry.begin("workspace");
    assert!(!registry.claim(&id, "foreign"));
    assert!(registry.claim(&id, "workspace"));
    assert!(!registry.claim(&id, "workspace"));
    assert!(!registry.cancel_unstarted(&id));
    assert!(registry.complete(&id, "workspace", "修改已提交".into()));
    assert_eq!(rx.await.unwrap(), "修改已提交");
}

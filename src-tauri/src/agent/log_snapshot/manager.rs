use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use parking_lot::Mutex;
use tokio::sync::{watch, RwLock, Semaphore};

use super::model::{SnapshotStatus, UpdateOptions, UpdateRecord, UpdateState};
use super::query::{LogReadRequest, LogReadResult, LogSearchRequest, LogSearchResult};
use super::store::SnapshotStore;
use super::update::{execute_update, UpdateContext, UpdateFailure, UpdateJob, UpdateReporter};

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
struct JobKey {
    workspace_id: String,
    snapshot_id: String,
}

#[derive(Clone)]
struct JobHandle {
    cancel: watch::Sender<bool>,
    progress: watch::Receiver<UpdateRecord>,
    raw_counter: Arc<AtomicU64>,
}

struct ManagerInner {
    jobs: Mutex<HashMap<JobKey, JobHandle>>,
    blocked_workspaces: Mutex<HashSet<String>>,
    leases: Mutex<HashMap<JobKey, Arc<RwLock<()>>>>,
    update_slots: Arc<Semaphore>,
}

#[derive(Clone)]
pub(crate) struct LogSnapshotManager {
    inner: Arc<ManagerInner>,
}

pub(crate) struct LogSnapshotBlock {
    manager: LogSnapshotManager,
    workspace_id: String,
    armed: bool,
}

impl LogSnapshotBlock {
    pub(crate) fn finish(mut self, keep_blocked: bool) {
        self.armed = false;
        if !keep_blocked {
            self.manager
                .inner
                .blocked_workspaces
                .lock()
                .remove(&self.workspace_id);
        }
    }
}

impl Drop for LogSnapshotBlock {
    fn drop(&mut self) {
        if self.armed {
            self.manager
                .inner
                .blocked_workspaces
                .lock()
                .remove(&self.workspace_id);
        }
    }
}

impl LogSnapshotManager {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(ManagerInner {
                jobs: Mutex::new(HashMap::new()),
                blocked_workspaces: Mutex::new(HashSet::new()),
                leases: Mutex::new(HashMap::new()),
                update_slots: Arc::new(Semaphore::new(1)),
            }),
        }
    }

    pub(crate) async fn start_update(
        &self,
        context: UpdateContext,
        mut options: UpdateOptions,
    ) -> Result<UpdateRecord, String> {
        options.validate()?;
        let snapshot_id = options
            .snapshot_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
        let key = JobKey {
            workspace_id: context.workspace_id.clone(),
            snapshot_id: snapshot_id.clone(),
        };
        if self.inner.jobs.lock().contains_key(&key) {
            return Err("该日志快照已有更新任务，请先查看状态或取消".into());
        }
        let store = SnapshotStore::new(context.session_root.clone(), context.workspace_id.clone());
        let current = load({
            let store = store.clone();
            let snapshot_id = snapshot_id.clone();
            move || store.load_current(&snapshot_id)
        })
        .await??;
        if options.sources.is_empty() && current.is_none() {
            return Err("首次创建快照必须提供 sources".into());
        }
        let generation = current
            .as_ref()
            .map(|current| current.generation + 1)
            .unwrap_or(1);
        options.snapshot_id = Some(snapshot_id.clone());
        let job_id = uuid::Uuid::new_v4().simple().to_string();
        let now = Utc::now().to_rfc3339();
        let record = UpdateRecord {
            job_id: job_id.clone(),
            snapshot_id: snapshot_id.clone(),
            generation,
            state: UpdateState::Queued,
            created_at: now.clone(),
            updated_at: now,
            raw_bytes: 0,
            file_count: 0,
            metrics: None,
            error: None,
        };
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (progress_tx, progress_rx) = watch::channel(record.clone());
        let reporter = UpdateReporter::new(record.clone(), progress_tx, cancel_rx.clone());
        let handle = JobHandle {
            cancel: cancel_tx,
            progress: progress_rx,
            raw_counter: reporter.raw_counter(),
        };
        {
            let blocked = self.inner.blocked_workspaces.lock();
            if blocked.contains(&context.workspace_id) {
                return Err("会话正在删除或清空，已拒绝新的日志快照更新".into());
            }
            let mut jobs = self.inner.jobs.lock();
            // contains_key 与 insert 之间必须原子（同一把锁内）：insert 会先覆盖
            // 旧 handle 再返回，若用返回值判断会把存活任务的句柄顶掉，致取消失效。
            if jobs.contains_key(&key) {
                return Err("该日志快照已有更新任务，请先查看状态或取消".into());
            }
            jobs.insert(key.clone(), handle);
        }
        let persisted = load({
            let store = store.clone();
            let record = record.clone();
            move || store.write_update(&record)
        })
        .await
        .and_then(|result| result);
        if let Err(error) = persisted {
            self.inner.jobs.lock().remove(&key);
            return Err(error);
        }
        let lease = self.lease(&key);
        let manager = self.clone();
        let job = UpdateJob {
            job_id,
            snapshot_id,
            generation,
            options,
            current,
            lease,
        };
        tokio::spawn(async move {
            manager.run_update(context, store, key, job, reporter).await;
        });
        Ok(record)
    }

    async fn run_update(
        self,
        context: UpdateContext,
        store: SnapshotStore,
        key: JobKey,
        job: UpdateJob,
        mut reporter: UpdateReporter,
    ) {
        let cancel = reporter.cancel();
        let slots = Arc::clone(&self.inner.update_slots);
        let snapshot_id = job.snapshot_id.clone();
        let job_id = job.job_id.clone();
        let result = tokio::select! {
            permit = slots.acquire_owned() => {
                match permit {
                    Ok(_permit) => {
                        use futures::FutureExt;
                        std::panic::AssertUnwindSafe(
                            execute_update(context, store.clone(), job, reporter.clone()),
                        )
                        .catch_unwind()
                        .await
                        .unwrap_or_else(|_| Err(UpdateFailure {
                            cancelled: false,
                            message: "日志更新任务发生内部错误，已按失败收口".into(),
                        }))
                    },
                    Err(error) => Err(UpdateFailure { cancelled: false, message: format!("日志更新队列已关闭：{error}") }),
                }
            }
            _ = crate::shared::cancel::wait_for_cancel(Some(cancel)) => {
                Err(UpdateFailure { cancelled: true, message: "日志快照更新在排队时已取消".into() })
            }
        };
        let failed = result.is_err();
        if failed {
            let _ = load({
                let store = store.clone();
                let snapshot_id = snapshot_id.clone();
                let job_id = job_id.clone();
                move || store.cleanup_staging(&snapshot_id, &job_id)
            })
            .await;
        }
        match result {
            Ok(manifest) => reporter.finish(Ok((manifest.raw_bytes, manifest.files.len()))),
            Err(error) => reporter.finish(Err(error)),
        }
        let record = reporter.record();
        let _ = load({
            let store = store.clone();
            move || store.write_update(&record)
        })
        .await;
        self.inner.jobs.lock().remove(&key);
        reporter.notify();
    }

    pub(crate) fn block_workspace(&self, workspace_id: &str) -> Result<LogSnapshotBlock, String> {
        let mut blocked = self.inner.blocked_workspaces.lock();
        if !blocked.insert(workspace_id.to_string()) {
            return Err("该会话已有日志快照清理任务在进行".into());
        }
        if self
            .inner
            .jobs
            .lock()
            .keys()
            .any(|key| key.workspace_id == workspace_id)
        {
            blocked.remove(workspace_id);
            return Err(
                "日志快照仍在更新，请先调用 ssh_log_snapshot_cancel 并等待任务结束后再删除或清空"
                    .into(),
            );
        }
        Ok(LogSnapshotBlock {
            manager: self.clone(),
            workspace_id: workspace_id.to_string(),
            armed: true,
        })
    }

    pub(crate) fn cancel(&self, workspace_id: &str, snapshot_id: &str) -> Result<bool, String> {
        super::model::validate_snapshot_id(snapshot_id)?;
        let key = JobKey {
            workspace_id: workspace_id.to_string(),
            snapshot_id: snapshot_id.to_string(),
        };
        Ok(self
            .inner
            .jobs
            .lock()
            .get(&key)
            .is_some_and(|job| job.cancel.send(true).is_ok()))
    }

    pub(crate) async fn status(
        &self,
        store: SnapshotStore,
        snapshot_id: &str,
        wait_secs: u64,
    ) -> Result<SnapshotStatus, String> {
        super::model::validate_snapshot_id(snapshot_id)?;
        let key = JobKey {
            workspace_id: store.workspace_id().to_string(),
            snapshot_id: snapshot_id.to_string(),
        };
        let active = self.inner.jobs.lock().get(&key).cloned();
        let mut active_record = active.as_ref().map(|job| job.progress.borrow().clone());
        if let (Some(job), Some(record)) = (&active, &active_record) {
            if wait_secs > 0 && !record.state.terminal() {
                let deadline = Instant::now() + Duration::from_secs(wait_secs.min(30));
                let mut receiver = job.progress.clone();
                loop {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    match tokio::time::timeout(remaining, receiver.changed()).await {
                        Err(_) => break,
                        // 发送端已关闭（任务句柄刚被移除）：不会再有新进度，避免空转到 deadline。
                        Ok(Err(_)) => break,
                        Ok(Ok(())) => {}
                    }
                    let current = receiver.borrow_and_update().clone();
                    let terminal = current.state.terminal();
                    active_record = Some(current);
                    if terminal {
                        break;
                    }
                }
            }
        }
        // 磁盘加载段持 lease 读锁：与 publish 的「rename + 指针切换 + 删旧代次」
        // 互斥，避免读到已被删除的旧代次 manifest。不包上面的 wait 等待循环。
        let lease = self.lease(&key);
        let _lease_guard = lease.read().await;
        let (current, stored_update) = load({
            let store = store.clone();
            let snapshot_id = snapshot_id.to_string();
            move || {
                let current = store.load_current(&snapshot_id)?;
                let update = store.load_update(&snapshot_id)?;
                Ok::<_, String>((current, update))
            }
        })
        .await??;
        let mut update = active_record.or(stored_update);
        let is_active = update
            .as_ref()
            .is_some_and(|record| active.is_some() && !record.state.terminal());
        if let Some(record) = &mut update {
            if is_active {
                if let Some(job) = &active {
                    record.raw_bytes = record
                        .raw_bytes
                        .max(job.raw_counter.load(Ordering::Relaxed));
                }
            } else if !record.state.terminal() {
                record.state = UpdateState::Failed;
                record.error =
                    Some("应用重启或后台任务中断；可重新调用 update，已发布代次不受影响".into());
            }
        }
        Ok(SnapshotStatus {
            snapshot_id: snapshot_id.to_string(),
            current,
            update,
            active: is_active,
        })
    }

    pub(crate) async fn list(&self, store: SnapshotStore) -> Result<Vec<SnapshotStatus>, String> {
        let mut ids = load({
            let store = store.clone();
            move || store.list_snapshot_ids()
        })
        .await??;
        let workspace_id = store.workspace_id().to_string();
        for key in self.inner.jobs.lock().keys() {
            if key.workspace_id == workspace_id && !ids.contains(&key.snapshot_id) {
                ids.push(key.snapshot_id.clone());
            }
        }
        ids.sort();
        ids.dedup();
        let mut statuses = Vec::with_capacity(ids.len());
        for id in ids {
            statuses.push(self.status(store.clone(), &id, 0).await?);
        }
        Ok(statuses)
    }

    pub(crate) async fn search(
        &self,
        store: SnapshotStore,
        request: LogSearchRequest,
        cancel: Option<watch::Receiver<bool>>,
    ) -> Result<LogSearchResult, String> {
        request.validate()?;
        let key = JobKey {
            workspace_id: store.workspace_id().to_string(),
            snapshot_id: request.snapshot_id.clone(),
        };
        let lease = self.lease(&key);
        let _guard = lease.read().await;
        let generation = load({
            let store = store.clone();
            let snapshot_id = request.snapshot_id.clone();
            move || store.load_current_generation(&snapshot_id)
        })
        .await??
        .ok_or_else(|| "日志快照尚无可用代次，请先完成 update".to_string())?;
        let mut result = super::search::execute(generation, request, cancel).await?;
        let snapshot_id = result.snapshot_id.clone();
        result.update = load({
            let store = store.clone();
            move || store.load_update(&snapshot_id)
        })
        .await??;
        Ok(result)
    }

    pub(crate) async fn read(
        &self,
        store: SnapshotStore,
        request: LogReadRequest,
    ) -> Result<LogReadResult, String> {
        request.validate()?;
        let key = JobKey {
            workspace_id: store.workspace_id().to_string(),
            snapshot_id: request.snapshot_id.clone(),
        };
        let lease = self.lease(&key);
        let _guard = lease.read().await;
        let generation = load({
            let store = store.clone();
            let snapshot_id = request.snapshot_id.clone();
            move || store.load_current_generation(&snapshot_id)
        })
        .await??
        .ok_or_else(|| "日志快照尚无可用代次，请先完成 update".to_string())?;
        load(move || super::read::execute(&store, &generation, request)).await?
    }

    fn lease(&self, key: &JobKey) -> Arc<RwLock<()>> {
        self.inner
            .leases
            .lock()
            .entry(key.clone())
            .or_insert_with(|| Arc::new(RwLock::new(())))
            .clone()
    }
}

async fn load<T: Send + 'static>(task: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|error| format!("日志快照本地任务失败：{error}"))
}

#[cfg(test)]
#[path = "manager/lifecycle_tests.rs"]
mod lifecycle_tests;
#[cfg(test)]
#[path = "manager/tests.rs"]
mod tests;

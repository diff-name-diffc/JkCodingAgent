//! 每个 Dispatcher 会话共享一个互斥执行域，UI 与 Agent 从同一入口取得执行权。
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::json;
use tokio::sync::{watch, Mutex as AsyncMutex, OwnedMutexGuard};
use tokio::time::{Duration, Instant};

use super::process::{BrowserProcess, EventSink};

struct ActiveOperation {
    exploring: bool,
    tool_call_id: Option<String>,
    cancel: watch::Sender<bool>,
}

#[derive(Default)]
pub(super) struct Session {
    gate: Arc<AsyncMutex<()>>,
    pub(super) process: Mutex<Option<Arc<BrowserProcess>>>,
    active: Mutex<Option<ActiveOperation>>,
    pub(super) preview: Arc<super::preview::PreviewRegistry>,
    generation: AtomicU64,
    stop_generation: AtomicU64,
    removing: AtomicBool,
}

impl Session {
    pub(super) fn mark_removing(&self) {
        self.removing.store(true, Ordering::Release);
    }
    pub(super) fn event_sink(self: &Arc<Self>, sink: EventSink) -> EventSink {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let session = Arc::downgrade(self);
        Arc::new(move |name, value| {
            if session
                .upgrade()
                .is_some_and(|session| session.generation.load(Ordering::Acquire) == generation)
            {
                sink(name, value);
            }
        })
    }

    pub(super) async fn enter(
        self: &Arc<Self>,
        exploring: bool,
        deadline: Instant,
        upstream: Option<watch::Receiver<bool>>,
    ) -> Result<Operation, String> {
        self.enter_with_identity(exploring, None, deadline, upstream)
            .await
    }

    pub(super) async fn enter_exploration(
        self: &Arc<Self>,
        tool_call_id: String,
        deadline: Instant,
        upstream: Option<watch::Receiver<bool>>,
    ) -> Result<Operation, String> {
        self.enter_with_identity(true, Some(tool_call_id), deadline, upstream)
            .await
    }

    async fn enter_with_identity(
        self: &Arc<Self>,
        exploring: bool,
        tool_call_id: Option<String>,
        deadline: Instant,
        upstream: Option<watch::Receiver<bool>>,
    ) -> Result<Operation, String> {
        let stop_generation = self.stop_generation.load(Ordering::Acquire);
        self.check_manual(exploring)?;
        let gate = tokio::select! {
            biased;
            _ = cancelled(upstream) => return Err("[cancelled] 浏览器操作已取消".into()),
            _ = tokio::time::sleep_until(deadline) => return Err("[timeout] 等待浏览器执行权超时".into()),
            gate = self.gate.clone().lock_owned() => gate,
        };
        self.check_manual(exploring)?;
        let (cancel, receiver) = watch::channel(false);
        {
            let mut active = self.active.lock();
            if self.stop_generation.load(Ordering::Acquire) != stop_generation {
                return Err("[cancelled] 浏览器排队操作已被停止".into());
            }
            *active = Some(ActiveOperation {
                exploring,
                tool_call_id,
                cancel,
            });
        }
        Ok(Operation {
            session: self.clone(),
            _gate: gate,
            receiver,
            completed: false,
        })
    }

    fn check_manual(&self, exploring: bool) -> Result<(), String> {
        if self.removing.load(Ordering::Acquire) {
            return Err("浏览器会话资源正在清理，请稍后重新操作".into());
        }
        if !exploring
            && self
                .active
                .lock()
                .as_ref()
                .is_some_and(|active| active.exploring)
        {
            Err("浏览器探索 Agent 正在执行；请先停止探索，再进行手动浏览操作".into())
        } else {
            Ok(())
        }
    }

    pub(super) fn process(&self) -> Option<Arc<BrowserProcess>> {
        self.process.lock().clone()
    }

    pub(super) fn cancel_exploration(&self, tool_call_id: &str) -> Result<(), String> {
        let active = self.active.lock();
        let operation = active
            .as_ref()
            .filter(|active| {
                active.exploring && active.tool_call_id.as_deref() == Some(tool_call_id)
            })
            .ok_or_else(|| "目标浏览器探索已结束或当前执行身份不匹配".to_string())?;
        operation.cancel.send_replace(true);
        Ok(())
    }

    pub(super) async fn stop(&self) -> Result<(), String> {
        {
            let active = self.active.lock();
            self.stop_generation.fetch_add(1, Ordering::AcqRel);
            if let Some(active) = active.as_ref() {
                active.cancel.send_replace(true);
            }
        }
        if let Some(process) = self.process() {
            process.terminate("浏览器已由用户停止").await?;
        }
        Ok(())
    }
}

pub(super) struct Operation {
    pub(super) session: Arc<Session>,
    _gate: OwnedMutexGuard<()>,
    receiver: watch::Receiver<bool>,
    completed: bool,
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(process) = self.process.get_mut() {
            process.abort("浏览器会话宿主已关闭");
        }
    }
}

impl Operation {
    pub(super) fn complete(&mut self) {
        self.completed = true;
    }

    pub(super) async fn run<T>(
        &self,
        deadline: Instant,
        upstream: Option<watch::Receiver<bool>>,
        run_id: Option<&str>,
        future: impl Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        let reason = tokio::select! {
            biased;
            _ = cancelled(upstream) => "[cancelled] 浏览器操作已取消",
            _ = cancelled(Some(self.receiver.clone())) => "[cancelled] 浏览器操作已停止",
            _ = tokio::time::sleep_until(deadline) => "[timeout] 浏览器操作超过总执行预算",
            result = future => return result,
        };
        if let Some(process) = self.session.process() {
            if let Some(run_id) = run_id {
                let _ = tokio::time::timeout(
                    Duration::from_secs(2),
                    process.request("cancel", json!({"runId": run_id})),
                )
                .await;
            }
            // 即使 cancel 返回成功也硬终止进程组，不以 IPC 响应替代动作终止。
            process
                .terminate(reason)
                .await
                .map_err(|error| format!("浏览器停止未确认（触发原因：{reason}）：{error}"))?;
        }
        Err(reason.into())
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        if !self.completed {
            if let Some(process) = self.session.process() {
                process.abort("浏览器执行提前结束，已终止进程树");
            }
        }
        self.session.active.lock().take();
    }
}

async fn cancelled(receiver: Option<watch::Receiver<bool>>) {
    let Some(mut receiver) = receiver else {
        return std::future::pending::<()>().await;
    };
    loop {
        if *receiver.borrow_and_update() {
            return;
        }
        if receiver.changed().await.is_err() {
            return;
        }
    }
}

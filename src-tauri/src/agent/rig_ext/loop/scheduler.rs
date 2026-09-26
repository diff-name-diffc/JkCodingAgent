//! 受管工具任务：worker 只结算 outbox，协调器负责唯一应答与结果交付。
use super::{
    batch::classify_tool_error,
    invocation::ToolInvocationContext,
    resources::{Claim, Resource, ResourceArbiter},
    support::{tool_error_text, tool_output_text},
    surface::ToolExecutionPolicy,
};
use crate::agent::rig_ext::{
    tool_result::{prepare::ResultPreparer, RigToolResultPolicy},
    tools::{run_record::prepare_arguments, spec::ToolSpec},
};
use crate::agent::{
    db::{
        tool_completions::{CompletionDraft, ToolCompletion},
        DispatcherDb, NewToolRun, ToolArtifactDraft, ToolRunTraceContext,
    },
    state::ActiveRunHandle,
};
use anyhow::{Context, Result};
use futures::FutureExt;
use rig::{
    message::ToolCall,
    tool::{PortableDynamicTool, ToolExecutionError},
};
mod claims;
mod dispatch;
use claims::claims;

use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::{
    sync::{watch, Semaphore},
    task::JoinSet,
    time::Instant,
};

static LEAF_LIMIT: OnceLock<Arc<Semaphore>> = OnceLock::new();

/// 取消/超时信号发出后，在途 worker 收敛的兜底上限。
///
/// 正常路径由工具自身的统一超时（`tools/spec.rs` 策略表最长 60 秒）与取消通道
/// 即时收口，远早于此即已收敛。选 600 秒是「最大统一超时的 10 倍」，只为兜住真正
/// 卡死的 worker，而不是把正常的慢工具误判为未收敛；到达上限不丢弃在途调用
/// （见 `hand_off`），只把监督权转交后台。
const SETTLE_CEILING: Duration = Duration::from_secs(600);

#[derive(Debug, thiserror::Error)]
#[error("Agent 运行已取消")]
pub(super) struct RuntimeCancelled;

pub(crate) struct TaskScheduler {
    runtime: Arc<super::runtime::ScopeRuntime>,
    pub db: DispatcherDb,
    pub workspace: String,
    pub run_id: String,
    pub scope_id: String,
    pub calls: BTreeMap<String, ToolCall>,
    jobs: JoinSet<Result<i64>>,
    pub ready: BTreeMap<i64, ToolCompletion>,
    pub recovered: std::collections::BTreeSet<i64>,
    cancel: watch::Receiver<bool>,
    prepare: ResultPreparer,
    budgets: Arc<super::budgets::RunBudgets>,
    delivery_permits: BTreeMap<String, tokio::sync::OwnedSemaphorePermit>,
    controls: BTreeMap<String, (u64, watch::Sender<bool>, Arc<std::sync::atomic::AtomicBool>)>,
    run_lease: Option<ActiveRunHandle>,
    events: tauri::ipc::Channel<crate::agent::rig_ext::events::AgentEvent>,
}

impl TaskScheduler {
    pub(crate) fn new(
        db: DispatcherDb,
        workspace: String,
        cancel: watch::Receiver<bool>,
        prepare: ResultPreparer,
        events: tauri::ipc::Channel<crate::agent::rig_ext::events::AgentEvent>,
    ) -> Self {
        let parent = ToolInvocationContext::current();
        let run_id = parent
            .as_ref()
            .map(|p| p.agent_run_id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let budgets = super::budgets::RunBudgets::shared(&run_id);
        let scope_id = uuid::Uuid::new_v4().to_string();
        let runtime =
            super::runtime::ScopeRuntime::new(&run_id, &scope_id, &workspace, events.clone());
        Self {
            db,
            workspace,
            run_id,
            scope_id,
            runtime,
            calls: BTreeMap::new(),
            jobs: JoinSet::new(),
            ready: BTreeMap::new(),
            recovered: Default::default(),
            cancel,
            prepare,
            budgets,
            delivery_permits: BTreeMap::new(),
            controls: BTreeMap::new(),
            events,
            run_lease: ActiveRunHandle::current(),
        }
    }
    pub(crate) async fn recover_completions(&mut self) -> Result<()> {
        let (db, workspace) = (self.db.clone(), self.workspace.clone());
        let rows =
            tokio::task::spawn_blocking(move || db.pending_root_tool_completions(&workspace))
                .await??;
        for mut completion in rows {
            // 历史致命故障作为观察交付，不取消新 run；旧用量不重复累计到本轮。
            completion.fatal = false;
            completion.usage_json = None;
            self.recovered.insert(completion.event_id);
            self.ready.insert(completion.event_id, completion);
        }
        Ok(())
    }

    pub(crate) fn pending(&self) -> bool {
        !self.calls.is_empty()
    }

    pub(crate) fn delivered(&mut self, task: &str) -> Option<ToolCall> {
        self.delivery_permits.remove(task);
        self.controls.remove(task);
        self.calls.remove(task)
    }

    async fn absorb(&mut self, event: i64) -> Result<()> {
        let (db, run, scope) = (self.db.clone(), self.run_id.clone(), self.scope_id.clone());
        let rows =
            tokio::task::spawn_blocking(move || db.pending_tool_completions(&run, &scope, event))
                .await??;
        for row in rows {
            self.ready.entry(row.event_id).or_insert(row);
        }
        Ok(())
    }
    pub(crate) async fn drive<F: std::future::Future>(&mut self, future: F) -> Result<F::Output> {
        anyhow::ensure!(
            !self.ready.values().any(|event| event.fatal),
            "工具发生致命故障，停止模型请求"
        );
        self.runtime.phase("deciding");
        tokio::pin!(future);
        let mut cancel = self.cancel.clone();
        loop {
            if *cancel.borrow() {
                return Err(RuntimeCancelled.into());
            }
            tokio::select! {
                result = &mut future => return Ok(result),
                _ = cancel.changed() => { return Err(RuntimeCancelled.into()); },
                job = self.jobs.join_next(), if !self.jobs.is_empty() => {
                    if let Some(job) = job { self.absorb(job.context("工具 worker 退出异常")??).await?; }
                    if self.ready.values().any(|event| event.fatal) {
                        if let Some(lease) = &self.run_lease { lease.request_cancel(); }
                        anyhow::bail!("工具发生致命故障，已停止模型请求与新工具调用");
                    }
                }
            }
        }
    }
    pub(crate) async fn collect_ready(&mut self) -> Result<()> {
        while let Some(job) = self.jobs.try_join_next() {
            self.absorb(job.context("工具 worker 退出异常")??).await?;
        }
        Ok(())
    }
    pub(crate) async fn window(&mut self, deadline: Instant) -> Result<()> {
        loop {
            tokio::select! {
                job = self.jobs.join_next(), if !self.jobs.is_empty() => {
                    if let Some(job) = job { self.absorb(job.context("工具 worker 退出异常")??).await?; }
                }
                _ = tokio::time::sleep_until(deadline) => break,
                else => break,
            }
            if self.jobs.is_empty() {
                break;
            }
        }
        Ok(())
    }
    pub(crate) async fn wait(&mut self) -> Result<()> {
        self.runtime.phase("waiting");
        self.collect_ready().await?;
        if !self.ready.is_empty() || !self.pending() {
            return Ok(());
        }
        let mut cancel = self.cancel.clone();
        if *cancel.borrow() {
            return Err(RuntimeCancelled.into());
        }
        tokio::select! {
            job = self.jobs.join_next() => {
                if let Some(job) = job { self.absorb(job.context("工具 worker 退出异常")??).await?; }
            },
            _ = cancel.changed() => return Err(RuntimeCancelled.into()),
        }
        Ok(())
    }
    pub(crate) fn cancel_all(&self) {
        for (_, sender, _) in self.controls.values() {
            sender.send_replace(true);
        }
    }

    pub(crate) fn cancel_queued(&self, round: Option<u64>) {
        for (dispatch_round, sender, active) in self.controls.values() {
            if round.is_none_or(|r| r == *dispatch_round)
                && active
                    .compare_exchange(
                        false,
                        true,
                        std::sync::atomic::Ordering::AcqRel,
                        std::sync::atomic::Ordering::Acquire,
                    )
                    .is_ok()
            {
                sender.send_replace(true);
            }
        }
    }
    /// 收敛全部在途 worker。正常路径受工具自身的统一超时/取消通道约束；整次收敛共用
    /// 一条 `SETTLE_CEILING` 上限（不随单个 worker 结算顺延，避免长尾把收尾拖成小时
    /// 级），到达上限时交接监督权并报「结算未确认」，让调用方明确知道本轮没有等到
    /// 全部结算（而不是无限等下去）。
    pub(crate) async fn drain(&mut self) -> Result<()> {
        self.drain_until(Instant::now() + SETTLE_CEILING).await
    }

    async fn drain_until(&mut self, deadline: Instant) -> Result<()> {
        let mut failure = None;
        loop {
            // 单独一条语句取值：让 `join_next` 的借用在本语句结束时收束，
            // 后续 absorb/交接才能再借 `self`。
            let next = tokio::time::timeout_at(deadline, self.jobs.join_next()).await;
            let job = match next {
                Ok(job) => job,
                Err(_) => {
                    // 不丢弃在途 worker：先让它们感知取消，再把监督权交给后台任务
                    // 跑完（结算与日志仍落在本 worker 内），调用方拿到明确错误。
                    let pending = self.jobs.len();
                    self.cancel_all();
                    hand_off(&mut self.jobs);
                    let unconfirmed = anyhow::anyhow!(
                        "工具结算未确认：发出取消信号后 {} 秒内仍有 {pending} 个工具调用未收敛，已转交后台继续等待其结算（结果可能在本轮之后交付）。",
                        SETTLE_CEILING.as_secs()
                    );
                    return Err(match failure {
                        Some(failure) => {
                            unconfirmed.context(format!("此前已有工具 worker 失败：{failure:#}"))
                        }
                        None => unconfirmed,
                    });
                }
            };
            let Some(job) = job else { break };
            match job {
                Ok(Ok(event)) => {
                    if let Err(error) = self.absorb(event).await {
                        failure = Some(error);
                    }
                }
                Ok(Err(error)) => failure = Some(error),
                Err(error) => failure = Some(error.into()),
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(())
    }
    pub(crate) async fn shutdown(&mut self) -> Result<()> {
        if self.jobs.is_empty() {
            return Ok(());
        }
        if !self.jobs.is_empty() {
            if let Some(lease) = &self.run_lease {
                lease.request_cancel();
            }
        }
        self.cancel_all();
        let runtime = self.runtime.clone();
        runtime.phase("cancelling");
        // 5 秒宽限只负责把 phase 切到 cleaning；总等待与 `drain` 共用同一上限。
        let drain = self.drain_until(Instant::now() + SETTLE_CEILING);
        tokio::pin!(drain);
        tokio::select! {
            result = &mut drain => result,
            _ = tokio::time::sleep(Duration::from_secs(5)) => {
                runtime.phase("cleaning");
                drain.await
            }
        }
    }
}

/// 把剩余 job 的监督权交给后台任务：worker 继续跑完并落下结算与日志，既不 abort
/// future（丢结算），也不阻塞调用方（对齐 `Drop` 的既有做法）。
fn hand_off(jobs: &mut JoinSet<Result<i64>>) {
    let mut jobs = std::mem::replace(jobs, JoinSet::new());
    if jobs.is_empty() {
        return;
    }
    tokio::spawn(async move {
        while let Some(result) = jobs.join_next().await {
            match result {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => eprintln!("工具结算失败：{error:#}"),
                Err(error) => eprintln!("清理工具 worker 失败：{error}"),
            }
        }
    });
}

impl Drop for TaskScheduler {
    fn drop(&mut self) {
        // JoinSet 默认 Drop 会 abort future；转移监督权，worker 租约保留到真实结算。
        if !self.jobs.is_empty() {
            if let Some(lease) = &self.run_lease {
                lease.request_cancel();
            }
        }
        hand_off(&mut self.jobs);
    }
}

async fn update_phase(db: &DispatcherDb, task: &str, phase: &str) -> Result<()> {
    let (db, task, phase) = (db.clone(), task.to_string(), phase.to_string());
    tokio::task::spawn_blocking(move || db.set_tool_task_phase(&task, &phase)).await?
}

fn is_composite(name: &str) -> bool {
    matches!(name, "call_sub_agent" | "run_tool_program")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 交接监督权不等于丢弃：后台任务仍会把 job 跑完（结算不丢），
    /// 且调度器不再持有它们（后续 Drop 不会 abort）。
    #[tokio::test]
    async fn hand_off_lets_detached_jobs_finish() {
        let settled = Arc::new(AtomicUsize::new(0));
        let (done, done_rx) = tokio::sync::oneshot::channel();
        let mut jobs = JoinSet::new();
        let flag = settled.clone();
        jobs.spawn(async move {
            flag.fetch_add(1, Ordering::SeqCst);
            done.send(()).ok();
            Ok(1)
        });
        hand_off(&mut jobs);
        assert!(jobs.is_empty(), "交接后调度器不得再持有 job");
        tokio::time::timeout(Duration::from_secs(5), done_rx)
            .await
            .expect("交接的 job 必须在后台跑完")
            .expect("worker 结算路径不得被丢弃");
        assert_eq!(settled.load(Ordering::SeqCst), 1);
    }

    /// 空集合交接是空操作（`Drop` 路径也会走到）。
    #[tokio::test]
    async fn hand_off_is_noop_for_empty_set() {
        let mut jobs: JoinSet<Result<i64>> = JoinSet::new();
        hand_off(&mut jobs);
        assert!(jobs.is_empty());
    }
}

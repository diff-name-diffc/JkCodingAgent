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

/// 叶子执行全局并发上限：生产为单机单应用的 4。测试进程内数十个 run 循环
/// 共用此静态信号量——4 个额度会被少数闸门慢测试挤占，让无关测试的 worker
/// 排队错过派发窗口（flaky 根源）；测试构建放宽到与每 run 叶子预算
/// （`budgets.rs` 的 32）同量级。
#[cfg(not(test))]
const LEAF_CONCURRENCY: usize = 4;
#[cfg(test)]
const LEAF_CONCURRENCY: usize = 32;

/// 任务级控制块：派发轮次、任务取消通道、是否已进入实际执行、入队时刻
/// （在途快照需要入队时刻计算已耗时）。
pub(crate) struct TaskControl {
    pub round: u64,
    pub cancel_tx: watch::Sender<bool>,
    pub active: Arc<std::sync::atomic::AtomicBool>,
    pub enqueued_at: Instant,
}

/// 在途任务快照行（注入每轮 preamble 的数据源；渲染见 `wait::render_pending_snapshot`）。
pub(crate) struct PendingTask {
    pub task_id: String,
    pub tool_name: String,
    pub running: bool,
    pub elapsed: Duration,
}

/// 取消/超时信号发出后，在途 worker 收敛的兜底上限（app_policy 的统一超时
/// 收口与此处共用同一上限，单一出处防止两层数值漂移）。
///
/// 正常路径由工具自身的统一超时（`tools/spec.rs` 策略表最长 60 秒）与取消通道
/// 即时收口，远早于此即已收敛。选 600 秒是「最大统一超时的 10 倍」，只为兜住真正
/// 卡死的 worker，而不是把正常的慢工具误判为未收敛；到达上限不丢弃在途调用
/// （见 `hand_off`），只把监督权转交后台。
pub(super) const SETTLE_CEILING: Duration = Duration::from_secs(600);

/// wait 唤醒条件满足后的合并宽限：把近同时完成的结算一次性吸收交付，
/// 避免 fan-out 批次被拆成多次唤醒（每次唤醒都是一轮完整模型请求）。
const WAIT_COALESCE_GRACE: Duration = Duration::from_millis(250);

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
    /// 本 run 内已结算（拿到终态 completion）的任务 id 全集：含失败/取消终态，
    /// 交付与观察确认后仍保留——wait 条件判定的唯一事实来源。
    settled: std::collections::BTreeSet<String>,
    cancel: watch::Receiver<bool>,
    prepare: ResultPreparer,
    budgets: Arc<super::budgets::RunBudgets>,
    delivery_permits: BTreeMap<String, tokio::sync::OwnedSemaphorePermit>,
    controls: BTreeMap<String, TaskControl>,
    run_lease: Option<ActiveRunHandle>,
    events: tauri::ipc::Channel<crate::agent::rig_ext::events::AgentEvent>,
    /// 可选的第二条事件通道：只有当宿主需要把叶子台账的 `ToolRunUpdated`
    /// 推给前端时才注入（ToolProgram 叶子宿主）。与 `events` 分开是刻意的：
    /// 叶子若把 `ToolStarted`/`ToolFinished` 发到真实通道，前端会按
    /// `tool_call_id`（形如 `program-call:step1`）新建顶层工具卡片。
    run_events: Option<tauri::ipc::Channel<crate::agent::rig_ext::events::AgentEvent>>,
}

impl TaskScheduler {
    pub(crate) fn new(
        db: DispatcherDb,
        workspace: String,
        cancel: watch::Receiver<bool>,
        prepare: ResultPreparer,
        events: tauri::ipc::Channel<crate::agent::rig_ext::events::AgentEvent>,
        run_events: Option<tauri::ipc::Channel<crate::agent::rig_ext::events::AgentEvent>>,
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
            settled: Default::default(),
            cancel,
            prepare,
            budgets,
            delivery_permits: BTreeMap::new(),
            controls: BTreeMap::new(),
            events,
            run_events,
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
            self.settled.insert(completion.tool_run_id.clone());
            self.ready.insert(completion.event_id, completion);
        }
        Ok(())
    }

    pub(crate) fn pending(&self) -> bool {
        !self.calls.is_empty()
    }

    pub(crate) fn delivered(&mut self, task: &str) {
        self.delivery_permits.remove(task);
        self.controls.remove(task);
        self.calls.remove(task);
    }

    /// 尚未结算（无终态 completion）的在途任务快照，供 wait 条件判定与
    /// preamble 在途清单使用。
    pub(crate) fn pending_tasks(&self) -> Vec<PendingTask> {
        let now = Instant::now();
        self.calls
            .iter()
            .filter(|(id, _)| !self.settled.contains(*id))
            .map(|(id, call)| {
                let control = self.controls.get(id);
                PendingTask {
                    task_id: id.clone(),
                    tool_name: call.function.name.clone(),
                    running: control.is_some_and(|control| {
                        control.active.load(std::sync::atomic::Ordering::Acquire)
                    }),
                    elapsed: control.map_or(Duration::ZERO, |control| {
                        now.saturating_duration_since(control.enqueued_at)
                    }),
                }
            })
            .collect()
    }

    /// 尚未结算的在途任务数（wait 结果 JSON 的 pending_count）。
    pub(crate) fn unsettled_count(&self) -> usize {
        self.calls
            .keys()
            .filter(|id| !self.settled.contains(*id))
            .count()
    }

    /// 按 task_id 取回并移除自身结算（共享调度器下每个宿主只取自己的那条）。
    ///
    /// `ready` 以 `event_id` 为键，不能整体 clear：同一调度器上其它叶子可能还有
    /// 未取走的结算。找不到即返回 None（调用方可据此判断 job 已空但结算未到）。
    pub(crate) fn take_completion(&mut self, task: &str) -> Option<ToolCompletion> {
        let event_id = self
            .ready
            .iter()
            .find(|(_, completion)| completion.tool_run_id == task)
            .map(|(event_id, _)| *event_id)?;
        self.ready.remove(&event_id)
    }

    /// 是否仍有在途 worker：宿主据此区分「还在跑」与「job 已空却缺结算」。
    pub(crate) fn has_jobs(&self) -> bool {
        !self.jobs.is_empty()
    }

    async fn absorb(&mut self, event: i64) -> Result<()> {
        let (db, run, scope) = (self.db.clone(), self.run_id.clone(), self.scope_id.clone());
        let rows =
            tokio::task::spawn_blocking(move || db.pending_tool_completions(&run, &scope, event))
                .await??;
        for row in rows {
            self.settled.insert(row.tool_run_id.clone());
            self.ready.entry(row.event_id).or_insert(row);
        }
        Ok(())
    }

    /// fatal 结算守卫：与 `drive` 同语义——发现致命故障即请求取消并停止循环。
    fn ensure_no_fatal(&self) -> Result<()> {
        if self.ready.values().any(|event| event.fatal) {
            if let Some(lease) = &self.run_lease {
                lease.request_cancel();
            }
            anyhow::bail!("工具发生致命故障，已停止模型请求与新工具调用");
        }
        Ok(())
    }

    /// 唤醒条件满足后的合并宽限：排空近同时完成的结算，一次唤醒交付一批，
    /// 避免 fan-out 批次被拆成多次唤醒。无在途 job 时立即返回。
    async fn coalesce_ready(&mut self) -> Result<()> {
        let deadline = Instant::now() + WAIT_COALESCE_GRACE;
        while !self.jobs.is_empty() {
            tokio::select! {
                job = self.jobs.join_next() => {
                    if let Some(job) = job { self.absorb(job.context("工具 worker 退出异常")??).await?; }
                }
                _ = tokio::time::sleep_until(deadline) => break,
            }
        }
        Ok(())
    }
    pub(crate) async fn drive<F: std::future::Future>(&mut self, future: F) -> Result<F::Output> {
        self.ensure_no_fatal()?;
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
                    self.ensure_no_fatal()?;
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
    /// 条件等待（`wait_for_tools` 显式等待与答复后隐式等待的唯一实现）：
    /// 挂起决策直到目标任务结算、取消或超时；事件驱动，无轮询。条件满足后
    /// 经合并宽限排空近同时完成的结算，一次唤醒交付一批。
    pub(crate) async fn wait_for(
        &mut self,
        condition: &super::wait::WaitCondition,
    ) -> Result<super::wait::WaitReason> {
        use super::wait::{WaitMode, WaitReason};
        self.runtime.phase("waiting");
        // 目标集：显式 task_ids 取与已知任务的交集（幻觉 id 被过滤）；缺省为
        // 当前全部未交付任务（已结算未交付的即刻满足条件，不会阻塞）。
        let targets: Vec<String> = match &condition.targets {
            Some(ids) => ids
                .iter()
                .filter(|id| self.calls.contains_key(*id))
                .cloned()
                .collect(),
            None => self.calls.keys().cloned().collect(),
        };
        let deadline = condition.timeout.map(|timeout| Instant::now() + timeout);
        let mut cancel = self.cancel.clone();
        loop {
            self.collect_ready().await?;
            self.ensure_no_fatal()?;
            if targets.is_empty() {
                return Ok(WaitReason::NoPendingTasks);
            }
            let met = match condition.mode {
                WaitMode::All => targets.iter().all(|id| self.settled.contains(id)),
                WaitMode::Any => targets.iter().any(|id| self.settled.contains(id)),
            };
            if met {
                self.coalesce_ready().await?;
                self.ensure_no_fatal()?;
                return Ok(WaitReason::Ready);
            }
            // 条件未满足但已无在途 worker（如结算监督已移交后台）：不悬空等待，
            // 交回决策权，由循环顶部交付现有结果。以独立原因如实上报，避免
            // 渲染出 reason=tools_ready 而 ready 为空的自相矛盾反馈。
            if self.jobs.is_empty() {
                return Ok(WaitReason::NoActiveWorkers);
            }
            if *cancel.borrow() {
                return Err(RuntimeCancelled.into());
            }
            tokio::select! {
                job = self.jobs.join_next() => {
                    if let Some(job) = job { self.absorb(job.context("工具 worker 退出异常")??).await?; }
                }
                _ = cancel.changed() => return Err(RuntimeCancelled.into()),
                _ = async {
                    match deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline).await,
                        None => std::future::pending().await,
                    }
                } => {
                    self.collect_ready().await?;
                    self.ensure_no_fatal()?;
                    return Ok(WaitReason::Timeout);
                }
            }
        }
    }
    pub(crate) fn cancel_all(&self) {
        for control in self.controls.values() {
            control.cancel_tx.send_replace(true);
        }
    }

    pub(crate) fn cancel_queued(&self, round: Option<u64>) {
        for control in self.controls.values() {
            if round.is_none_or(|r| r == control.round)
                && control
                    .active
                    .compare_exchange(
                        false,
                        true,
                        std::sync::atomic::Ordering::AcqRel,
                        std::sync::atomic::Ordering::Acquire,
                    )
                    .is_ok()
            {
                control.cancel_tx.send_replace(true);
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
        if let Some(lease) = &self.run_lease {
            lease.request_cancel();
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

    fn completion(event_id: i64, tool_run_id: &str) -> ToolCompletion {
        ToolCompletion {
            event_id,
            tool_run_id: tool_run_id.into(),
            tool_name: "read_file".into(),
            tool_call_id: format!("call-{tool_run_id}"),
            dispatch_round: 1,
            agent_run_id: "run".into(),
            scope_id: "scope".into(),
            status: "succeeded".into(),
            error_kind: None,
            fatal: false,
            retryable: false,
            display_content: "ok".into(),
            context_payload: "ok".into(),
            result_mode: "inline".into(),
            usage_json: None,
            delivery_message_id: None,
            observed_request_step: None,
        }
    }

    /// 共享调度器下 `take_completion` 只取自己的那条：其它叶子的结算必须留在
    /// `ready` 里等各自宿主取走（整体 clear 会把它们丢掉）。
    #[tokio::test]
    async fn take_completion_removes_only_the_target_entry() {
        let dir = crate::test_util::TempDirGuard::new("rig-scheduler-take");
        let db = DispatcherDb::new(dir.path().join("jkbot.sqlite3")).expect("open temp db");
        let (_cancel_tx, cancel) = watch::channel(false);
        let mut scheduler = TaskScheduler::new(
            db,
            "workspace".into(),
            cancel,
            crate::agent::rig_ext::tool_result::prepare::raw_preparer(),
            tauri::ipc::Channel::new(|_| Ok(())),
            None,
        );
        scheduler.ready.insert(1, completion(1, "first"));
        scheduler.ready.insert(2, completion(2, "second"));

        let taken = scheduler.take_completion("first").expect("目标结算可取走");
        assert_eq!(taken.tool_run_id, "first");
        assert_eq!(taken.event_id, 1);
        assert_eq!(scheduler.ready.len(), 1, "另一条结算必须仍留在 ready 中");
        assert_eq!(
            scheduler.ready.get(&2).map(|c| c.tool_run_id.as_str()),
            Some("second")
        );
        assert!(
            scheduler.take_completion("first").is_none(),
            "同一结算不得被取两次"
        );
        assert!(!scheduler.has_jobs(), "未入队时没有在途 job");
    }

    use super::super::wait::{WaitCondition, WaitMode, WaitReason};

    /// 返回的 cancel sender 必须由调用方持有：sender 一掉线，接收端的
    /// `changed()` 立即按「通道关闭 = 取消」收口。
    #[allow(clippy::type_complexity)]
    fn wait_scheduler(
        dir_name: &str,
    ) -> (
        crate::test_util::TempDirGuard,
        watch::Sender<bool>,
        TaskScheduler,
    ) {
        let dir = crate::test_util::TempDirGuard::new(dir_name);
        let db = DispatcherDb::new(dir.path().join("jkbot.sqlite3")).expect("open temp db");
        let (cancel_tx, cancel) = watch::channel(false);
        let scheduler = TaskScheduler::new(
            db,
            "workspace".into(),
            cancel,
            crate::agent::rig_ext::tool_result::prepare::raw_preparer(),
            tauri::ipc::Channel::new(|_| Ok(())),
            None,
        );
        (dir, cancel_tx, scheduler)
    }

    fn pending_call(task_id: &str) -> ToolCall {
        ToolCall::from_wire(
            format!("call-{task_id}"),
            rig::message::ToolFunction {
                name: "read_file".into(),
                arguments: serde_json::json!({}),
            },
        )
    }

    /// 无在途任务时立即返回 NoPendingTasks（竞态兜底：工具面只在有未结算
    /// 任务时才注入 wait_for_tools，此处守卫幻觉调用与竞态）。
    #[tokio::test]
    async fn wait_for_without_tasks_returns_no_pending() {
        let (_dir, _cancel_tx, mut scheduler) = wait_scheduler("rig-wait-empty");
        let reason = scheduler
            .wait_for(&WaitCondition::any_pending())
            .await
            .expect("空等待应立即返回");
        assert_eq!(reason, WaitReason::NoPendingTasks);
    }

    /// 目标子集全部结算即满足 join 条件——即便仍有其它在途任务也不阻塞。
    #[tokio::test]
    async fn wait_for_subset_returns_ready_when_targets_settled() {
        let (_dir, _cancel_tx, mut scheduler) = wait_scheduler("rig-wait-subset");
        for id in ["a", "b", "c"] {
            scheduler.calls.insert(id.into(), pending_call(id));
        }
        scheduler.settled.insert("a".into());
        scheduler.settled.insert("b".into());
        let condition = WaitCondition {
            targets: Some(["a".to_string(), "b".to_string()].into_iter().collect()),
            mode: WaitMode::All,
            timeout: None,
        };
        let reason = scheduler
            .wait_for(&condition)
            .await
            .expect("目标子集已结算应满足条件");
        assert_eq!(reason, WaitReason::Ready);
        // any 模式：任一目标结算即满足。
        let condition = WaitCondition {
            targets: Some(["c".to_string(), "b".to_string()].into_iter().collect()),
            mode: WaitMode::Any,
            timeout: None,
        };
        let reason = scheduler
            .wait_for(&condition)
            .await
            .expect("任一结算即满足");
        assert_eq!(reason, WaitReason::Ready);
    }

    /// 显式幻觉 id 被过滤：全部不在已知任务中时视为无在途任务。
    #[tokio::test]
    async fn wait_for_unknown_targets_returns_no_pending() {
        let (_dir, _cancel_tx, mut scheduler) = wait_scheduler("rig-wait-unknown");
        let condition = WaitCondition {
            targets: Some(["ghost".to_string()].into_iter().collect()),
            mode: WaitMode::All,
            timeout: None,
        };
        let reason = scheduler
            .wait_for(&condition)
            .await
            .expect("幻觉 id 被过滤");
        assert_eq!(reason, WaitReason::NoPendingTasks);
    }

    /// 声明 timeout_secs 到点返回 Timeout 并交回决策权（虚拟时间下立即推进）。
    #[tokio::test(start_paused = true)]
    async fn wait_for_timeout_returns_timeout() {
        let (_dir, _cancel_tx, mut scheduler) = wait_scheduler("rig-wait-timeout");
        scheduler.calls.insert("a".into(), pending_call("a"));
        // 永不结算的在途 worker：保证等待真正挂起，直到超时兜底。
        scheduler
            .jobs
            .spawn(async { std::future::pending::<Result<i64>>().await });
        let condition = WaitCondition {
            targets: None,
            mode: WaitMode::All,
            timeout: Some(Duration::from_secs(1)),
        };
        let reason = scheduler.wait_for(&condition).await.expect("超时应返回");
        assert_eq!(reason, WaitReason::Timeout);
    }
}

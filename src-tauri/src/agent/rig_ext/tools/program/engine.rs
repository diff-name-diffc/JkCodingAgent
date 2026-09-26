//! `ExecutionEngine` 的并发调度部分：并发许可获取、parallel 分支扇出/合并、
//! 以及错误/取消/超时的小助手。与 `executor.rs` 的节点执行主流程分开，
//! 控制单文件规模。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};
use futures::FutureExt;
use tokio::time::Instant;

use super::super::ast::ProgramNode;
use super::super::error::{ProgramError, ProgramErrorKind};
use super::super::support::{cancel_wait, ensure_environment_budget, is_stopped, mark_stopped};
use super::super::value::StepEnvironment;
use super::{ExecutionEngine, FlowError};

pub(super) struct BranchOutcome {
    pub index: usize,
    pub environment: StepEnvironment,
    pub completed_steps: Vec<String>,
    pub result: Result<Option<serde_json::Value>, FlowError>,
}

impl ExecutionEngine<'_> {
    pub(super) async fn acquire_permit(
        &self,
        id: &str,
        tool: &str,
        stop_signals: &[Arc<AtomicBool>],
    ) -> Result<tokio::sync::OwnedSemaphorePermit, FlowError> {
        if is_stopped(stop_signals) {
            return Err(FlowError::Stopped);
        }
        if self.cancel_requested() {
            return Err(self.fail(self.cancelled_error(Some(id), Some(tool)), stop_signals));
        }
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            self.mark_deadline_reached();
            return Err(self.fail(self.deadline_error(Some(id), Some(tool)), stop_signals));
        }
        let acquire = self.semaphore.clone().acquire_owned();
        tokio::pin!(acquire);
        let deadline = tokio::time::sleep(remaining);
        tokio::pin!(deadline);
        let cancel = cancel_wait(self.cancel_rx.clone());
        tokio::pin!(cancel);
        tokio::select! {
            biased;
            permit = &mut acquire => permit.map_err(|_| {
                self.fail(
                    ProgramError::new(
                        ProgramErrorKind::Internal,
                        "ToolProgram 全局并发信号量已关闭",
                    )
                    .for_step(id, tool),
                    stop_signals,
                )
            }),
            _ = &mut deadline => {
                self.mark_deadline_reached();
                if is_stopped(stop_signals) {
                    Err(FlowError::Stopped)
                } else {
                    Err(self.fail(self.deadline_error(Some(id), Some(tool)), stop_signals))
                }
            }
            _ = &mut cancel => {
                if is_stopped(stop_signals) {
                    Err(FlowError::Stopped)
                } else {
                    Err(self.fail(self.cancelled_error(Some(id), Some(tool)), stop_signals))
                }
            }
        }
    }

    pub(super) async fn execute_parallel(
        &self,
        branches: &[ProgramNode],
        environment: &mut StepEnvironment,
        completed_steps: &mut Vec<String>,
        inherited_stops: &[Arc<AtomicBool>],
    ) -> Result<(), FlowError> {
        let entry_environment = environment.clone();
        let parallel_stop = Arc::new(AtomicBool::new(false));
        let mut child_stops = inherited_stops.to_vec();
        child_stops.push(parallel_stop.clone());

        let futures = FuturesUnordered::new();
        for (index, branch) in branches.iter().enumerate() {
            let mut branch_environment = entry_environment.clone();
            let mut branch_completed = Vec::new();
            let branch_stops = child_stops.clone();
            futures.push(
                async move {
                    let result = self
                        .execute_node(
                            branch,
                            &mut branch_environment,
                            &mut branch_completed,
                            &branch_stops,
                        )
                        .await;
                    BranchOutcome {
                        index,
                        environment: branch_environment,
                        completed_steps: branch_completed,
                        result,
                    }
                }
                .boxed(),
            );
        }

        let mut outcomes: Vec<Option<BranchOutcome>> = std::iter::repeat_with(|| None)
            .take(branches.len())
            .collect();
        tokio::pin!(futures);
        while let Some(outcome) = futures.next().await {
            if outcome.result.is_err() {
                parallel_stop.store(true, Ordering::Release);
            }
            let index = outcome.index;
            outcomes[index] = Some(outcome);
        }

        let mut primary_error: Option<(usize, ProgramError)> = None;
        let mut saw_stopped = false;
        for outcome in outcomes.iter().filter_map(Option::as_ref) {
            completed_steps.extend(outcome.completed_steps.iter().cloned());
            match &outcome.result {
                Ok(None) => {}
                Ok(Some(_)) => {
                    primary_error.get_or_insert_with(|| {
                        (
                            outcome.index,
                            ProgramError::new(
                                ProgramErrorKind::Internal,
                                "parallel branch 意外产生 return 值",
                            ),
                        )
                    });
                }
                Err(FlowError::Program(error)) => {
                    if primary_error
                        .as_ref()
                        .is_none_or(|(index, _)| outcome.index < *index)
                    {
                        primary_error = Some((outcome.index, error.clone()));
                    }
                }
                Err(FlowError::Stopped) => saw_stopped = true,
            }
        }
        self.sort_completed_steps(completed_steps);

        if let Some((_, error)) = primary_error {
            return Err(FlowError::Program(error));
        }
        if saw_stopped {
            return Err(FlowError::Program(ProgramError::new(
                ProgramErrorKind::Internal,
                "parallel 已停止，但未找到触发停止的失败分支",
            )));
        }

        // 只有全部分支成功后才按声明顺序合并。这样环境顺序与完成时间无关，
        // 且失败分支不会把部分结果泄漏给后续 sequence。
        for outcome in outcomes.into_iter().flatten() {
            for (step_id, envelope) in outcome.environment {
                if !entry_environment.contains_key(&step_id) {
                    environment.insert(step_id, envelope);
                }
            }
            ensure_environment_budget(environment, self.limits.max_environment_bytes)
                .map_err(FlowError::Program)?;
        }
        Ok(())
    }

    pub(super) fn fail(&self, error: ProgramError, stop_signals: &[Arc<AtomicBool>]) -> FlowError {
        mark_stopped(stop_signals);
        FlowError::Program(error)
    }

    pub(super) fn mark_deadline_reached(&self) {
        self.deadline_reached.store(true, Ordering::Release);
    }

    pub(super) fn cancel_requested(&self) -> bool {
        self.cancel_rx.as_ref().is_some_and(|rx| *rx.borrow())
    }

    pub(super) fn deadline_error(&self, id: Option<&str>, tool: Option<&str>) -> ProgramError {
        let error = ProgramError::new(
            ProgramErrorKind::DeadlineExceeded,
            format!(
                "ToolProgram 达到整体 wall-time 上限 {} 秒；已停止调度新调用，在途调用等待真实结算后返回",
                self.limits.max_wall_time_secs,
            ),
        );
        match (id, tool) {
            (Some(id), Some(tool)) => error.for_step(id, tool),
            _ => error,
        }
    }

    pub(super) fn cancelled_error(&self, id: Option<&str>, tool: Option<&str>) -> ProgramError {
        let error = ProgramError::new(
            ProgramErrorKind::Cancelled,
            "ToolProgram 收到外层取消信号；已停止调度新调用，在途调用等待真实结算后返回",
        );
        match (id, tool) {
            (Some(id), Some(tool)) => error.for_step(id, tool),
            _ => error,
        }
    }

    pub(super) fn sort_completed_steps(&self, completed_steps: &mut Vec<String>) {
        completed_steps.sort_by_key(|id| self.sequences.get(id).copied().unwrap_or(u64::MAX));
        completed_steps.dedup();
    }
}

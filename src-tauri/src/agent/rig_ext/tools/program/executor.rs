//! ToolProgram 执行器：按 AST 编排调用数据面工具。
//!
//! 迁移自旧 `agent/tools/program/executor.rs`：「按名调用工具」的接缝由旧
//! `CapabilityBroker`（`CapabilityInvocation` + `ToolResult`/`ToolStatus`）
//! 改为注入的 `DataPlane`（按名查找 `PortableDynamicTool` 并 `execute`）。
//! 并发上限、wall-time、drain 收敛、预算守卫与停止信号语义逐条保留；
//! 审计 invocation id / outer_call_id 不再由执行器构造（新 runtime 策略层
//! 负责每次调用的审计与门禁）；外层取消直接消费 run 级 `cancel_rx`
//! （数据面工具与程序共享同一 run 取消源，无需再桥接独立取消通道）。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use futures::FutureExt;
use serde_json::Value;
use tokio::sync::{watch, Semaphore};
use tokio::time::Instant;

use rig::tool::{ToolExecutionError, ToolOutput};

use super::ast::ProgramNode;
use super::error::{ProgramError, ProgramErrorKind};
use super::support::{
    cancel_wait, child_error, collect_call_sequences, ensure_environment_budget,
    ensure_json_budget, is_stopped, mark_stopped, render_value, success_envelope,
    validate_execution_limits,
};
use super::validate::{ProgramLimits, ValidatedProgram};
use super::value::{resolve_template, StepEnvironment};
use super::DataPlane;

/// 程序执行成功结果：return 值 + 按声明序排序的已完成步骤。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProgramSuccess {
    pub value: Value,
    pub completed_steps: Vec<String>,
}

/// rig 工具面入口：执行已验证的 ToolProgram。
///
/// 成功返回渲染后的 return 值文本（对齐旧 `output_for_llm` 口径：字符串原样、
/// 其余 JSON 序列化）；失败映射为带分类 code 的 `ToolExecutionError`
/// （见 `super::program_error_tool_error`）。
pub(crate) async fn execute_program(
    program: &ValidatedProgram,
    plane: &DataPlane,
    limits: &ProgramLimits,
    cancel_rx: Option<watch::Receiver<bool>>,
) -> Result<ToolOutput, ToolExecutionError> {
    let success = execute_program_inner(program, plane, limits, cancel_rx)
        .await
        .map_err(super::program_error_tool_error)?;
    render_value(&success.value)
        .map(ToolOutput::text)
        .map_err(|error| {
            super::program_error_tool_error(error.with_completed_steps(success.completed_steps))
        })
}

/// 执行已经过静态验证的 ToolProgram。
///
/// wall-time 到达或收到外层取消时停止调度新调用，并给在途调用最多
/// `max_drain_time_ms` 的协作收敛窗口（不直接 drop future 伪装已终止）。
pub(crate) async fn execute_program_inner(
    program: &ValidatedProgram,
    plane: &DataPlane,
    limits: &ProgramLimits,
    cancel_rx: Option<watch::Receiver<bool>>,
) -> Result<ProgramSuccess, ProgramError> {
    validate_execution_limits(limits)?;
    let sequences = collect_call_sequences(&program.program().root);
    let Some(deadline) = Instant::now().checked_add(Duration::from_secs(limits.max_wall_time_secs))
    else {
        return Err(ProgramError::new(
            ProgramErrorKind::Internal,
            "ToolProgram wall-time 上限无法表示",
        ));
    };
    let engine = ExecutionEngine {
        plane,
        limits,
        sequences,
        semaphore: Arc::new(Semaphore::new(limits.max_concurrency)),
        deadline,
        deadline_reached: Arc::new(AtomicBool::new(false)),
        cancel_rx,
    };
    let mut environment = StepEnvironment::new();
    let mut completed_steps = Vec::new();

    let outcome = engine
        .execute_node(
            &program.program().root,
            &mut environment,
            &mut completed_steps,
            &[],
        )
        .await;
    engine.sort_completed_steps(&mut completed_steps);

    match outcome {
        Ok(Some(value)) => Ok(ProgramSuccess {
            value,
            completed_steps,
        }),
        Ok(None) => Err(
            ProgramError::new(
                ProgramErrorKind::Internal,
                "ToolProgram 执行结束但没有产生 return 值",
            )
            .with_completed_steps(completed_steps),
        ),
        Err(FlowError::Program(error)) => Err(error.with_completed_steps(completed_steps)),
        Err(FlowError::Stopped) => Err(ProgramError::new(
            ProgramErrorKind::Internal,
            "ToolProgram 顶层执行被并行停止信号意外中断",
        )
        .with_completed_steps(completed_steps)),
    }
}

struct ExecutionEngine<'a> {
    plane: &'a DataPlane,
    limits: &'a ProgramLimits,
    sequences: BTreeMap<String, u64>,
    semaphore: Arc<Semaphore>,
    deadline: Instant,
    deadline_reached: Arc<AtomicBool>,
    cancel_rx: Option<watch::Receiver<bool>>,
}

#[derive(Debug)]
enum FlowError {
    Program(ProgramError),
    /// 兄弟分支已经失败，本分支尚未开始的调用被正常抑制。
    Stopped,
}

enum Settled {
    Finished(Result<ToolOutput, ToolExecutionError>),
    Deadline,
    Cancelled,
}

#[path = "engine.rs"]
mod engine;

impl ExecutionEngine<'_> {
    fn execute_node<'node>(
        &'node self,
        node: &'node ProgramNode,
        environment: &'node mut StepEnvironment,
        completed_steps: &'node mut Vec<String>,
        stop_signals: &'node [Arc<AtomicBool>],
    ) -> BoxFuture<'node, Result<Option<Value>, FlowError>> {
        async move {
            if is_stopped(stop_signals) {
                return Err(FlowError::Stopped);
            }
            if self.deadline_reached.load(Ordering::Acquire) || Instant::now() >= self.deadline {
                self.mark_deadline_reached();
                let error = self.deadline_error(None, None);
                mark_stopped(stop_signals);
                return Err(FlowError::Program(error));
            }
            if self.cancel_requested() {
                let error = self.cancelled_error(None, None);
                mark_stopped(stop_signals);
                return Err(FlowError::Program(error));
            }

            let result = match node {
                ProgramNode::Call {
                    id,
                    tool,
                    arguments,
                } => self
                    .execute_call(
                        id,
                        tool,
                        arguments,
                        environment,
                        completed_steps,
                        stop_signals,
                    )
                    .await
                    .map(|()| None),
                ProgramNode::Sequence { steps } => {
                    for step in steps {
                        if let Some(value) = self
                            .execute_node(step, environment, completed_steps, stop_signals)
                            .await?
                        {
                            return Ok(Some(value));
                        }
                    }
                    Ok(None)
                }
                ProgramNode::Parallel { branches } => self
                    .execute_parallel(branches, environment, completed_steps, stop_signals)
                    .await
                    .map(|()| None),
                ProgramNode::Return { value } => {
                    let resolved =
                        resolve_template(value, environment).map_err(FlowError::Program)?;
                    ensure_json_budget(&resolved, self.limits.max_return_bytes, "return 结果")
                        .map_err(FlowError::Program)?;
                    Ok(Some(resolved))
                }
            };

            if result.is_err() {
                mark_stopped(stop_signals);
            }
            result
        }
        .boxed()
    }

    async fn execute_call(
        &self,
        id: &str,
        tool: &str,
        arguments: &Value,
        environment: &mut StepEnvironment,
        completed_steps: &mut Vec<String>,
        stop_signals: &[Arc<AtomicBool>],
    ) -> Result<(), FlowError> {
        if is_stopped(stop_signals) {
            return Err(FlowError::Stopped);
        }
        if self.cancel_requested() {
            return Err(self.fail(self.cancelled_error(Some(id), Some(tool)), stop_signals));
        }
        let resolved_arguments = resolve_template(arguments, environment)
            .map_err(|error| self.fail(error.for_step(id, tool), stop_signals))?;
        if !resolved_arguments.is_object() {
            return Err(self.fail(
                ProgramError::new(
                    ProgramErrorKind::Validation,
                    format!("步骤 '{id}' 解析后的 arguments 不是 JSON object"),
                )
                .for_step(id, tool),
                stop_signals,
            ));
        }
        ensure_json_budget(
            &resolved_arguments,
            self.limits.max_resolved_arguments_bytes,
            &format!("步骤 '{id}' 的解析后参数"),
        )
        .map_err(|error| self.fail(error.for_step(id, tool), stop_signals))?;

        let permit = self.acquire_permit(id, tool, stop_signals).await?;
        if is_stopped(stop_signals) {
            drop(permit);
            return Err(FlowError::Stopped);
        }

        let Some(tool_impl) = self.plane.get(tool) else {
            drop(permit);
            return Err(self.fail(
                ProgramError::new(
                    ProgramErrorKind::Internal,
                    format!("步骤 '{id}' 的数据面工具 '{tool}' 在执行时缺失"),
                )
                .for_step(id, tool),
                stop_signals,
            ));
        };
        let sequence_present = self.sequences.contains_key(id);
        if !sequence_present {
            drop(permit);
            return Err(self.fail(
                ProgramError::new(
                    ProgramErrorKind::Internal,
                    format!("步骤 '{id}' 缺少声明序号"),
                )
                .for_step(id, tool),
                stop_signals,
            ));
        }

        let future = tool_impl.execute(resolved_arguments);
        tokio::pin!(future);
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        let deadline = tokio::time::sleep(remaining);
        tokio::pin!(deadline);
        let cancel = cancel_wait(self.cancel_rx.clone());
        tokio::pin!(cancel);

        let settled = tokio::select! {
            biased;
            result = future.as_mut() => Settled::Finished(result),
            _ = &mut deadline => Settled::Deadline,
            _ = &mut cancel => Settled::Cancelled,
        };
        let result = match settled {
            Settled::Finished(result) => result,
            Settled::Deadline => {
                // wall-time 到达后禁止启动任何新调用，但必须继续等待当前数据面
                // 调用收敛；直接 drop future 会把“已终止”伪装成事实。
                self.mark_deadline_reached();
                mark_stopped(stop_signals);
                let drained = tokio::time::timeout(
                    Duration::from_millis(self.limits.max_drain_time_ms),
                    future.as_mut(),
                )
                .await;
                if drained.is_ok() {
                    completed_steps.push(id.to_string());
                }
                drop(permit);
                return Err(FlowError::Program(self.deadline_error(Some(id), Some(tool))));
            }
            Settled::Cancelled => {
                // 取消与 wall-time 同一收敛纪律：停止调度新调用，在途调用等待
                // 有界 drain（数据面工具消费同一 run 取消源，会自行终止）。
                mark_stopped(stop_signals);
                let drained = tokio::time::timeout(
                    Duration::from_millis(self.limits.max_drain_time_ms),
                    future.as_mut(),
                )
                .await;
                if drained.is_ok() {
                    completed_steps.push(id.to_string());
                }
                drop(permit);
                return Err(FlowError::Program(self.cancelled_error(Some(id), Some(tool))));
            }
        };
        drop(permit);
        completed_steps.push(id.to_string());

        let output = match result {
            Ok(output) => output,
            Err(error) => {
                return Err(self.fail(child_error(id, tool, &error), stop_signals));
            }
        };

        let envelope = success_envelope(&output);
        ensure_json_budget(
            &envelope,
            self.limits.max_step_envelope_bytes,
            &format!("步骤 '{id}' 的结果 envelope"),
        )
        .map_err(|error| self.fail(error.for_step(id, tool), stop_signals))?;

        environment.insert(id.to_string(), envelope);
        ensure_environment_budget(environment, self.limits.max_environment_bytes)
            .map_err(|error| self.fail(error.for_step(id, tool), stop_signals))?;
        Ok(())
    }

}

#[cfg(test)]
#[path = "executor_tests.rs"]
mod tests;

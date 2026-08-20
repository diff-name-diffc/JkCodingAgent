use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use futures::FutureExt;
use serde_json::{json, Value};
use tokio::sync::{watch, Semaphore};
use tokio::time::Instant;

use super::ast::{ProgramNode, TOOL_PROGRAM_VERSION};
use super::error::{ProgramError, ProgramErrorKind};
use super::validate::{ProgramLimits, ValidatedProgram};
use super::value::{resolve_template, StepEnvironment};
use crate::agent::tools::{CapabilityBroker, CapabilityInvocation, ToolResult, ToolStatus};

/// ToolProgram 执行器所需的最小宿主能力接口。
///
/// 生产环境由 `CapabilityBroker` 实现；测试可注入纯内存 Broker。执行器不会
/// 接触 ToolRegistry、ToolContext、数据库或 LLM 消息。
pub trait ProgramBroker: Sync {
    fn invoke_capability(&self, invocation: CapabilityInvocation) -> BoxFuture<'_, ToolResult>;
}

impl ProgramBroker for CapabilityBroker<'_> {
    fn invoke_capability(&self, invocation: CapabilityInvocation) -> BoxFuture<'_, ToolResult> {
        Box::pin(CapabilityBroker::invoke(self, invocation))
    }
}

/// 执行已经过静态验证的 ToolProgram，并只返回一个聚合 ToolResult。
///
/// `outer_call_id` 用于派生稳定的内部 invocation id；每个 call 还会按 AST
/// 声明顺序获得从 1 开始的 sequence，供 Broker 构建稳定审计树。
#[cfg(test)]
pub async fn execute_program<B: ProgramBroker + ?Sized>(
    program: &ValidatedProgram,
    broker: &B,
    outer_call_id: &str,
    limits: &ProgramLimits,
) -> ToolResult {
    execute_program_inner(program, broker, outer_call_id, limits, None).await
}

/// 生产运行时入口：除返回 deadline 错误外，还会在 wall-time 到达的同一时刻
/// 向 Broker 链路发送取消，让在途工具进入协作终止与有界收敛。
pub async fn execute_program_with_cancellation<B: ProgramBroker + ?Sized>(
    program: &ValidatedProgram,
    broker: &B,
    outer_call_id: &str,
    limits: &ProgramLimits,
    cancel_tx: watch::Sender<bool>,
) -> ToolResult {
    execute_program_inner(program, broker, outer_call_id, limits, Some(cancel_tx)).await
}

async fn execute_program_inner<B: ProgramBroker + ?Sized>(
    program: &ValidatedProgram,
    broker: &B,
    outer_call_id: &str,
    limits: &ProgramLimits,
    cancel_tx: Option<watch::Sender<bool>>,
) -> ToolResult {
    if let Err(error) = validate_execution_inputs(outer_call_id, limits) {
        return program_error_result(error);
    }
    let sequences = collect_call_sequences(&program.program().root);
    let Some(deadline) = Instant::now().checked_add(Duration::from_secs(limits.max_wall_time_secs))
    else {
        return program_error_result(ProgramError::new(
            ProgramErrorKind::Internal,
            "ToolProgram wall-time 上限无法表示",
        ));
    };
    let engine = ExecutionEngine {
        broker,
        outer_call_id,
        limits,
        sequences,
        semaphore: Arc::new(Semaphore::new(limits.max_concurrency)),
        deadline,
        deadline_reached: Arc::new(AtomicBool::new(false)),
        cancel_tx,
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
        Ok(Some(value)) => success_result(value, completed_steps),
        Ok(None) => program_error_result(
            ProgramError::new(
                ProgramErrorKind::Internal,
                "ToolProgram 执行结束但没有产生 return 值",
            )
            .with_completed_steps(completed_steps),
        ),
        Err(FlowError::Program(error)) => {
            program_error_result(error.with_completed_steps(completed_steps))
        }
        Err(FlowError::Stopped) => program_error_result(
            ProgramError::new(
                ProgramErrorKind::Internal,
                "ToolProgram 顶层执行被并行停止信号意外中断",
            )
            .with_completed_steps(completed_steps),
        ),
    }
}

struct ExecutionEngine<'a, B: ProgramBroker + ?Sized> {
    broker: &'a B,
    outer_call_id: &'a str,
    limits: &'a ProgramLimits,
    sequences: BTreeMap<String, u64>,
    semaphore: Arc<Semaphore>,
    deadline: Instant,
    deadline_reached: Arc<AtomicBool>,
    cancel_tx: Option<watch::Sender<bool>>,
}

#[derive(Debug)]
enum FlowError {
    Program(ProgramError),
    /// 兄弟分支已经失败，本分支尚未开始的调用被正常抑制。
    Stopped,
}

struct BranchOutcome {
    index: usize,
    environment: StepEnvironment,
    completed_steps: Vec<String>,
    result: Result<Option<Value>, FlowError>,
}

impl<'a, B: ProgramBroker + ?Sized> ExecutionEngine<'a, B> {
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

        let sequence = self.sequences.get(id).copied().ok_or_else(|| {
            self.fail(
                ProgramError::new(
                    ProgramErrorKind::Internal,
                    format!("步骤 '{id}' 缺少声明序号"),
                )
                .for_step(id, tool),
                stop_signals,
            )
        })?;
        let invocation = CapabilityInvocation::tool_program(
            format!("{}:{sequence}", self.outer_call_id),
            id,
            sequence,
            tool,
            resolved_arguments,
        );
        let future = self.broker.invoke_capability(invocation);
        tokio::pin!(future);
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        let deadline = tokio::time::sleep(remaining);
        tokio::pin!(deadline);

        let result = tokio::select! {
            biased;
            result = future.as_mut() => result,
            _ = &mut deadline => {
                // wall-time 到达后禁止启动任何新调用，但必须继续等待当前 Broker
                // 收敛；直接 drop future 会把“已终止”伪装成事实。
                self.mark_deadline_reached();
                mark_stopped(stop_signals);
                let settled = tokio::time::timeout(
                    Duration::from_millis(self.limits.max_drain_time_ms),
                    future.as_mut(),
                )
                .await;
                if settled.is_ok() {
                    completed_steps.push(id.to_string());
                }
                drop(permit);
                return Err(FlowError::Program(self.deadline_error(Some(id), Some(tool))));
            }
        };
        drop(permit);
        completed_steps.push(id.to_string());

        match result.status {
            ToolStatus::Success => {}
            ToolStatus::RecoverableError => {
                return Err(self.fail(
                    child_error(ProgramErrorKind::ChildRecoverable, id, tool, &result),
                    stop_signals,
                ));
            }
            ToolStatus::FatalError => {
                return Err(self.fail(
                    child_error(ProgramErrorKind::ChildFatal, id, tool, &result),
                    stop_signals,
                ));
            }
            ToolStatus::Cancelled => {
                return Err(self.fail(
                    child_error(ProgramErrorKind::Cancelled, id, tool, &result),
                    stop_signals,
                ));
            }
        }

        let envelope = result_envelope(result);
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

    async fn acquire_permit(
        &self,
        id: &str,
        tool: &str,
        stop_signals: &[Arc<AtomicBool>],
    ) -> Result<tokio::sync::OwnedSemaphorePermit, FlowError> {
        if is_stopped(stop_signals) {
            return Err(FlowError::Stopped);
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
        }
    }

    async fn execute_parallel(
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

    fn fail(&self, error: ProgramError, stop_signals: &[Arc<AtomicBool>]) -> FlowError {
        mark_stopped(stop_signals);
        FlowError::Program(error)
    }

    fn mark_deadline_reached(&self) {
        self.deadline_reached.store(true, Ordering::Release);
        if let Some(cancel_tx) = &self.cancel_tx {
            let _ = cancel_tx.send(true);
        }
    }

    fn deadline_error(&self, id: Option<&str>, tool: Option<&str>) -> ProgramError {
        let error = ProgramError::new(
            ProgramErrorKind::DeadlineExceeded,
            format!(
                "ToolProgram 达到整体 wall-time 上限 {} 秒；已停止调度新调用，在途调用最多等待 {} ms 收敛",
                self.limits.max_wall_time_secs,
                self.limits.max_drain_time_ms,
            ),
        );
        match (id, tool) {
            (Some(id), Some(tool)) => error.for_step(id, tool),
            _ => error,
        }
    }

    fn sort_completed_steps(&self, completed_steps: &mut Vec<String>) {
        completed_steps.sort_by_key(|id| self.sequences.get(id).copied().unwrap_or(u64::MAX));
        completed_steps.dedup();
    }
}

fn collect_call_sequences(root: &ProgramNode) -> BTreeMap<String, u64> {
    fn visit(node: &ProgramNode, next: &mut u64, sequences: &mut BTreeMap<String, u64>) {
        match node {
            ProgramNode::Call { id, .. } => {
                sequences.insert(id.clone(), *next);
                *next += 1;
            }
            ProgramNode::Sequence { steps } => {
                for step in steps {
                    visit(step, next, sequences);
                }
            }
            ProgramNode::Parallel { branches } => {
                for branch in branches {
                    visit(branch, next, sequences);
                }
            }
            ProgramNode::Return { .. } => {}
        }
    }

    let mut sequences = BTreeMap::new();
    let mut next = 1;
    visit(root, &mut next, &mut sequences);
    sequences
}

fn result_envelope(mut result: ToolResult) -> Value {
    let output = result.output_for_llm();
    json!({
        "status": match result.status {
            ToolStatus::Success => "success",
            ToolStatus::RecoverableError => "recoverable_error",
            ToolStatus::FatalError => "fatal_error",
            ToolStatus::Cancelled => "cancelled",
        },
        "data": result.data.take().unwrap_or(Value::Null),
        "output": output,
        "metadata": std::mem::take(&mut result.metadata),
    })
}

fn child_error(kind: ProgramErrorKind, id: &str, tool: &str, result: &ToolResult) -> ProgramError {
    ProgramError::new(
        kind,
        format!(
            "步骤 '{id}' 调用工具 '{tool}' 失败：{}",
            result.output_for_llm()
        ),
    )
    .for_step(id, tool)
}

fn ensure_json_budget(value: &Value, maximum: usize, label: &str) -> Result<(), ProgramError> {
    let size = serde_json::to_vec(value)
        .map_err(|error| {
            ProgramError::new(
                ProgramErrorKind::Internal,
                format!("计算 {label} JSON 大小时失败：{error}"),
            )
        })?
        .len();
    if size > maximum {
        return Err(ProgramError::new(
            ProgramErrorKind::LimitExceeded,
            format!("{label} 为 {size} 字节，超过上限 {maximum} 字节"),
        ));
    }
    Ok(())
}

fn ensure_environment_budget(
    environment: &StepEnvironment,
    maximum: usize,
) -> Result<(), ProgramError> {
    let size = serde_json::to_vec(environment)
        .map_err(|error| {
            ProgramError::new(
                ProgramErrorKind::Internal,
                format!("计算 ToolProgram 环境大小失败：{error}"),
            )
        })?
        .len();
    if size > maximum {
        return Err(ProgramError::new(
            ProgramErrorKind::LimitExceeded,
            format!("ToolProgram 环境为 {size} 字节，超过上限 {maximum} 字节"),
        ));
    }
    Ok(())
}

fn is_stopped(stop_signals: &[Arc<AtomicBool>]) -> bool {
    stop_signals
        .iter()
        .any(|signal| signal.load(Ordering::Acquire))
}

fn mark_stopped(stop_signals: &[Arc<AtomicBool>]) {
    for signal in stop_signals {
        signal.store(true, Ordering::Release);
    }
}

fn validate_execution_inputs(
    outer_call_id: &str,
    limits: &ProgramLimits,
) -> Result<(), ProgramError> {
    if outer_call_id.trim().is_empty() {
        return Err(ProgramError::new(
            ProgramErrorKind::Validation,
            "ToolProgram outer_call_id 不能为空",
        ));
    }
    let invalid_budget = limits.max_concurrency == 0
        || limits.max_concurrency > Semaphore::MAX_PERMITS
        || limits.max_resolved_arguments_bytes == 0
        || limits.max_step_envelope_bytes == 0
        || limits.max_environment_bytes == 0
        || limits.max_return_bytes == 0
        || limits.max_wall_time_secs == 0
        || limits.max_drain_time_ms == 0
        || limits.max_drain_time_ms > 5_000;
    if invalid_budget {
        return Err(ProgramError::new(
            ProgramErrorKind::Internal,
            "ToolProgram 执行预算必须为正数，且并发数不得超过 Semaphore 上限",
        ));
    }
    Ok(())
}

fn success_result(value: Value, completed_steps: Vec<String>) -> ToolResult {
    let context = match render_value(&value) {
        Ok(context) => context,
        Err(error) => return program_error_result(error.with_completed_steps(completed_steps)),
    };
    let mut result = ToolResult::success_data(value, context.clone(), context);
    result.metadata = json!({
        "toolProgram": {
            "version": TOOL_PROGRAM_VERSION,
            "completedSteps": completed_steps,
        }
    });
    result
}

/// 将静态验证或运行期 ProgramError 映射为统一的外层 ToolResult。
pub fn program_error_result(error: ProgramError) -> ToolResult {
    let kind = error.kind;
    let message = format!("错误：ToolProgram 执行失败：{}", error.message);
    let mut result = match kind {
        ProgramErrorKind::ChildFatal | ProgramErrorKind::Internal => {
            ToolResult::fatal_error(message)
        }
        ProgramErrorKind::Cancelled => ToolResult::cancelled(message),
        ProgramErrorKind::Parse
        | ProgramErrorKind::Validation
        | ProgramErrorKind::LimitExceeded
        | ProgramErrorKind::InvalidReference
        | ProgramErrorKind::PolicyDenied
        | ProgramErrorKind::ChildRecoverable
        | ProgramErrorKind::DeadlineExceeded => ToolResult::recoverable_error(message),
    };
    result.metadata = json!({
        "toolProgram": {
            "version": TOOL_PROGRAM_VERSION,
            "error": error,
        }
    });
    result
}

fn render_value(value: &Value) -> Result<String, ProgramError> {
    match value {
        Value::String(text) => Ok(text.clone()),
        other => serde_json::to_string(other).map_err(|error| {
            ProgramError::new(
                ProgramErrorKind::Internal,
                format!("ToolProgram return 序列化失败：{error}"),
            )
        }),
    }
}

#[cfg(test)]
#[path = "executor_tests.rs"]
mod tests;

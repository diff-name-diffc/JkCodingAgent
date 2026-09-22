//! ToolProgram 执行器的纯函数助手：声明序收集、结果 envelope、预算守卫、
//! 停止信号与取消等待。迁移自旧 `agent/tools/program/executor.rs` 的同名助手。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::{watch, Semaphore};

use rig::tool::{ToolErrorKind, ToolExecutionError, ToolOutput};

use super::ast::ProgramNode;
use super::error::{ProgramError, ProgramErrorKind};
use super::validate::ProgramLimits;
use super::value::StepEnvironment;

/// 按 AST 声明顺序为每个 call 分配从 1 开始的序号，用于把并发完成的
/// completed_steps 稳定排序回声明序（旧实现把序号交给 Broker 构建审计树；
/// rig 形态下审计由 runtime 策略层负责，序号仅供本执行器内部排序）。
pub(super) fn collect_call_sequences(root: &ProgramNode) -> BTreeMap<String, u64> {
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

/// 数据面调用成功后的步骤 envelope。保持旧 DSL 引用面：`/data` 为结构化
/// JSON 结果（工具返回单 JSON 块时），`/output` 为渲染文本，`/metadata`
/// 恒为空对象（PortableDynamicTool 无元数据通道；旧 `metadata.broker.*`
/// 审计字段随旧 Broker 一起退役）。
pub(super) fn success_envelope(output: &ToolOutput) -> Value {
    json!({
        "status": "success",
        "data": output.as_json().cloned().unwrap_or(Value::Null),
        "output": output.render(),
        "metadata": {},
    })
}

/// 子调用失败分类：rig 侧只有取消是可确定语义（`ToolErrorKind::Cancelled`）；
/// 其余错误（含门禁拒绝 refusal / permission_denied）一律按 ChildRecoverable
/// 处理——程序中止、外层模型可修正后重试，对齐旧 `ToolStatus::RecoverableError`
/// 的语义。旧 `ChildFatal` 不再由子调用产生（保留枚举用于外层映射完整性）。
pub(super) fn child_error(id: &str, tool: &str, error: &ToolExecutionError) -> ProgramError {
    let kind = if error.kind() == ToolErrorKind::Cancelled {
        ProgramErrorKind::Cancelled
    } else {
        ProgramErrorKind::ChildRecoverable
    };
    ProgramError::new(
        kind,
        format!(
            "步骤 '{id}' 调用工具 '{tool}' 失败：{}",
            error.model_output().render()
        ),
    )
    .for_step(id, tool)
}

pub(super) fn ensure_json_budget(
    value: &Value,
    maximum: usize,
    label: &str,
) -> Result<(), ProgramError> {
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

pub(super) fn ensure_environment_budget(
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

pub(super) fn is_stopped(stop_signals: &[Arc<AtomicBool>]) -> bool {
    stop_signals
        .iter()
        .any(|signal| signal.load(Ordering::Acquire))
}

pub(super) fn mark_stopped(stop_signals: &[Arc<AtomicBool>]) {
    for signal in stop_signals {
        signal.store(true, Ordering::Release);
    }
}

pub(super) fn validate_execution_limits(limits: &ProgramLimits) -> Result<(), ProgramError> {
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

pub(super) fn render_value(value: &Value) -> Result<String, ProgramError> {
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

/// 等待外层取消信号：None 时永不就绪；通道关闭按取消处理（fail-closed，
/// 对齐旧宿主把父取消桥接进程序取消通道的语义）。
pub(super) async fn cancel_wait(cancel_rx: Option<watch::Receiver<bool>>) {
    let Some(mut rx) = cancel_rx else {
        return std::future::pending::<()>().await;
    };
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            return;
        }
    }
}

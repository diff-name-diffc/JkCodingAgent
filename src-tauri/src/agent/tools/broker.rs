use std::time::Duration;
use std::{io, mem};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::ipc::Channel;
use tokio::sync::watch;

use super::{
    CapabilitySet, ToolContext, ToolRegistry, ToolResult, ToolRunFinishUpdate, ToolRuntime,
};
use crate::agent::db::{DispatcherDb, ToolRunTraceContext};
use crate::agent::llm::RequestedToolCall;
use crate::agent::run_loop::AgentEvent;
use crate::agent::ssh_review::{review_shell_command, CommandReviewPayload, CommandReviewTarget};
use crate::agent::tools::ToolSafety;

mod audit;
mod authorization;
mod execution;

use execution::{
    cancelled_before_start, enforce_tool_program_result_budget, with_broker_metadata,
    with_policy_metadata,
};

/// ToolProgram 单步 envelope 的宿主侧预检预算，低于 executor 的 256 KiB
/// 硬上限，为状态与固定字段留出余量。超限结果不做含糊截断：完整原文先进入
/// child artifact，再返回可恢复错误要求缩小参数。
const MAX_TOOL_PROGRAM_RESULT_BYTES: usize = 192 * 1024;
// 上层 ToolProgram 还会施加 5 秒 drain 硬边界；Broker 必须略早收口，
// 给审计落库和任务调度留出余量，避免两个同长 timeout 在边界互相竞速。
const CANCELLATION_SETTLE_GRACE: Duration = Duration::from_secs(4);

/// 调用来自哪个受信任的运行层。该值只用于审计，不参与授权；授权唯一取决于
/// `CapabilitySet`，避免调用方伪造 origin 提权。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum InvocationOrigin {
    Model,
    ToolProgram,
}

impl InvocationOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::ToolProgram => "tool_program",
        }
    }
}

/// 一次宿主能力调用。内部运行时只能提交数据，不持有 ToolContext、凭据或
/// ToolRegistry；Broker 在宿主侧把调用与实际上下文拼合。
#[derive(Debug, Clone)]
pub struct CapabilityInvocation {
    pub id: String,
    pub name: String,
    pub arguments: Value,
    pub origin: InvocationOrigin,
    pub step_id: Option<String>,
    pub sequence: u64,
}

impl CapabilityInvocation {
    pub fn model(id: impl Into<String>, name: impl Into<String>, arguments: Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
            origin: InvocationOrigin::Model,
            step_id: None,
            sequence: 0,
        }
    }

    pub fn tool_program(
        id: impl Into<String>,
        step_id: impl Into<String>,
        sequence: u64,
        name: impl Into<String>,
        arguments: Value,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
            origin: InvocationOrigin::ToolProgram,
            step_id: Some(step_id.into()),
            sequence,
        }
    }
}

#[derive(Clone, Copy)]
pub struct BrokerAudit<'a> {
    pub db: &'a DispatcherDb,
    pub workspace_id: &'a str,
    pub on_event: &'a Channel<AgentEvent>,
    pub parent_run_id: &'a str,
}

/// 所有真实工具执行的唯一策略入口。
///
/// 这里统一完成能力校验、Schema 参数准备、路径上下文规范化、取消与超时；
/// ToolProgram、普通 Agent、子智能体和图宿主都只能经该入口触达注册表。
pub struct CapabilityBroker<'a> {
    registry: &'a ToolRegistry,
    capabilities: CapabilitySet,
    context: &'a ToolContext,
    include_dynamic: bool,
    cancel_rx: Option<watch::Receiver<bool>>,
    audit: Option<BrokerAudit<'a>>,
}

impl<'a> CapabilityBroker<'a> {
    pub fn new(
        registry: &'a ToolRegistry,
        capabilities: CapabilitySet,
        context: &'a ToolContext,
    ) -> Self {
        Self {
            registry,
            capabilities,
            context,
            include_dynamic: true,
            cancel_rx: None,
            audit: None,
        }
    }

    pub fn include_dynamic(mut self, include_dynamic: bool) -> Self {
        self.include_dynamic = include_dynamic;
        self
    }

    pub fn with_cancellation(mut self, cancel_rx: watch::Receiver<bool>) -> Self {
        self.cancel_rx = Some(cancel_rx);
        self
    }

    pub fn with_audit(mut self, audit: BrokerAudit<'a>) -> Self {
        self.audit = Some(audit);
        self
    }

    pub async fn invoke(&self, invocation: CapabilityInvocation) -> ToolResult {
        if !self.capabilities.contains(&invocation.name) {
            return with_broker_metadata(
                ToolResult::recoverable_error(format!(
                    "错误：禁止调用工具 '{}'；该能力未授予当前执行上下文。",
                    invocation.name
                )),
                &invocation,
            );
        }

        let Some(spec) = self.registry.spec_by_name(
            &self.context.mcp_scope,
            &invocation.name,
            self.include_dynamic,
        ) else {
            return with_broker_metadata(
                ToolResult::recoverable_error(format!(
                    "错误：未找到工具 '{}'，能力目录可能已过期。",
                    invocation.name
                )),
                &invocation,
            );
        };

        let child_run_id = match self.start_child_run(&invocation).await {
            Ok(run_id) => run_id,
            Err(error) => {
                return with_broker_metadata(
                    ToolResult::fatal_error(format!(
                        "错误：创建内部工具调用审计记录失败：{error:#}"
                    )),
                    &invocation,
                )
            }
        };
        if self
            .cancel_rx
            .as_ref()
            .is_some_and(crate::agent::common::cancellation_requested)
        {
            return self
                .finish_child_run(
                    child_run_id,
                    &invocation,
                    with_broker_metadata(cancelled_before_start(&invocation), &invocation),
                )
                .await;
        }

        let input = match self.registry.prepare_input(
            &self.context.mcp_scope,
            &invocation.name,
            &invocation.arguments,
            self.include_dynamic,
        ) {
            Ok(input) => input,
            Err(result) => {
                let mut result = with_policy_metadata(
                    with_broker_metadata(*result, &invocation),
                    json!({
                        "safety": spec.safety,
                        "decision": "not_evaluated",
                        "reason": "invalid_arguments",
                    }),
                );
                if self.audit.is_some() && spec.result_policy.persist_raw_artifact {
                    result.ensure_raw_artifact(&invocation.name);
                }
                return self
                    .finish_child_run(child_run_id, &invocation, result)
                    .await;
            }
        };

        let policy_decision = match self
            .authorize(&invocation, &spec, &input.effective_arguments)
            .await
        {
            Ok(decision) => decision,
            Err(result) => {
                let result = with_policy_metadata(
                    with_broker_metadata(result, &invocation),
                    json!({
                        "safety": spec.safety,
                        "decision": "deny",
                    }),
                );
                return self
                    .finish_child_run(child_run_id, &invocation, result)
                    .await;
            }
        };

        let mut execution_context = self.context.clone();
        execution_context.current_tool_call_id = Some(invocation.id.clone());
        execution_context.current_tool_spec_hash = Some(spec.fingerprint());
        // 每次调用都持有独立的取消通道。外层取消和统一超时只向这个通道
        // 发信号，真实工具因此有机会终止子进程/阻塞工作，而不是仅仅丢弃
        // Future 后把副作用留在后台继续运行。
        let initially_cancelled = self
            .cancel_rx
            .as_ref()
            .is_some_and(|cancel_rx| *cancel_rx.borrow());
        let (execution_cancel_tx, execution_cancel_rx) = watch::channel(initially_cancelled);
        execution_context.cancel_rx = Some(execution_cancel_rx);
        execution_context.normalize_paths();

        let execution = self.registry.execute_input(input, &execution_context);
        tokio::pin!(execution);

        let result = if !spec.execution.unified_timeout || spec.execution.timeout_secs == 0 {
            self.await_without_timeout(
                &invocation,
                spec.execution.cancellable,
                &execution_cancel_tx,
                execution,
            )
            .await
        } else {
            self.await_with_timeout(
                &invocation,
                spec.execution.cancellable,
                Duration::from_secs(spec.execution.timeout_secs),
                &execution_cancel_tx,
                execution,
            )
            .await
        };

        let mut result =
            with_policy_metadata(with_broker_metadata(result, &invocation), policy_decision);
        let raw_audited = self.audit.is_some() && spec.result_policy.persist_raw_artifact;
        if raw_audited {
            result.ensure_raw_artifact(&invocation.name);
        }
        if invocation.origin == InvocationOrigin::ToolProgram {
            result = enforce_tool_program_result_budget(result, &invocation, raw_audited);
        }
        self.finish_child_run(child_run_id, &invocation, result)
            .await
    }
}

#[cfg(test)]
mod tests;

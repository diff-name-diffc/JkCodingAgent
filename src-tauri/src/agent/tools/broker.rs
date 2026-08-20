use std::path::Path;
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
    workspace: &'a Path,
    capabilities: CapabilitySet,
    context: &'a ToolContext,
    include_dynamic: bool,
    cancel_rx: Option<watch::Receiver<bool>>,
    audit: Option<BrokerAudit<'a>>,
}

impl<'a> CapabilityBroker<'a> {
    pub fn new(
        registry: &'a ToolRegistry,
        workspace: &'a Path,
        capabilities: CapabilitySet,
        context: &'a ToolContext,
    ) -> Self {
        Self {
            registry,
            workspace,
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

        let Some(spec) =
            self.registry
                .spec_by_name(self.workspace, &invocation.name, self.include_dynamic)
        else {
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
            self.workspace,
            &invocation.name,
            &invocation.arguments,
            self.include_dynamic,
        ) {
            Ok(input) => input,
            Err(result) => {
                let mut result = with_policy_metadata(
                    with_broker_metadata(result, &invocation),
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

    async fn authorize(
        &self,
        invocation: &CapabilityInvocation,
        spec: &super::ToolSpec,
        effective_arguments: &Value,
    ) -> Result<Value, ToolResult> {
        let resource_scope =
            self.authorize_resource_scope(invocation, spec, effective_arguments)?;
        let mut decision = match spec.safety {
            ToolSafety::Safe => json!({
                "safety": "safe",
                "decision": "allow",
            }),
            ToolSafety::Dangerous => {
                return Err(ToolResult::fatal_error(format!(
                    "错误：工具 '{}' 被策略标记为 dangerous，运行时默认拒绝执行。",
                    invocation.name
                )))
            }
            ToolSafety::ReviewRequired if spec.review_self_managed => {
                // 命令类工具（exec / local_zsh / ssh_exec）在内部携带完整目标环境
                // 上下文做 fail-closed 审查（目标服务器 / 执行目录、stdin、
                // 服务器级审查开关）。broker 若再做一次通用 JSON 审查，反而会用
                // 较弱的结论覆盖工具自身的审查语义，因此这里直接放行到工具内部。
                json!({
                    "safety": "review_required",
                    "decision": "allow",
                    "review": "self_managed",
                })
            }
            ToolSafety::ReviewRequired => {
                let Some(config) = self.context.ssh_review.as_ref() else {
                    return Err(ToolResult::recoverable_error(format!(
                        "错误：工具 '{}' 需要安全审查，但当前执行上下文未配置审查模型，已按 fail-closed 拒绝。",
                        invocation.name
                    )));
                };
                let arguments = serde_json::to_string(effective_arguments).map_err(|error| {
                    ToolResult::recoverable_error(format!(
                        "错误：工具 '{}' 的审查参数无法序列化：{error}",
                        invocation.name
                    ))
                })?;
                let payload = CommandReviewPayload {
                    intent: self.context.session_title.clone(),
                    task: self.context.user_task.clone().unwrap_or_default(),
                    target: CommandReviewTarget::AgentTool {
                        workspace_path: self.workspace.display().to_string(),
                        tool_name: invocation.name.clone(),
                        provider: spec.provider.clone(),
                        policy_summary: format!(
                            "readonly={}, workspaceBound={}, network={}, mutatesFilesystem={}, mutatesExternalState={}, resourceScope={}",
                            spec.access.readonly,
                            spec.access.workspace_bound,
                            spec.access.requires_network,
                            spec.access.mutates_filesystem,
                            spec.access.mutates_external_state,
                            resource_scope,
                        ),
                    },
                    command: arguments,
                    stdin: None,
                };
                let verdict = review_shell_command(config, &payload)
                    .await
                    .map_err(|error| {
                        ToolResult::recoverable_error(format!(
                            "错误：工具 '{}' 安全审查失败，已拒绝执行：{error}",
                            invocation.name
                        ))
                    })?;
                if !verdict.allowed {
                    return Err(ToolResult::recoverable_error(format!(
                        "错误：工具 '{}' 已被安全审查拦截：{}",
                        invocation.name, verdict.reason
                    )));
                }
                json!({
                    "safety": "review_required",
                    "decision": "allow",
                    "review": "approved",
                })
            }
        };
        if let Value::Object(policy) = &mut decision {
            policy.insert("resourceScope".to_string(), resource_scope);
        }
        Ok(decision)
    }

    fn authorize_resource_scope(
        &self,
        invocation: &CapabilityInvocation,
        spec: &super::ToolSpec,
        effective_arguments: &Value,
    ) -> Result<Value, ToolResult> {
        if !self.capabilities.has_write_restriction() {
            return Ok(json!({ "mode": "unrestricted", "decision": "allow" }));
        }

        if matches!(invocation.name.as_str(), "write_file" | "edit_file") {
            let path = effective_arguments
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ToolResult::recoverable_error(format!(
                        "错误：工具 '{}' 缺少可验证的 path 参数，资源授权拒绝执行。",
                        invocation.name
                    ))
                })?;
            if !self
                .capabilities
                .permits_workspace_write(self.workspace, path)
            {
                return Err(ToolResult::recoverable_error(format!(
                    "错误：工具 '{}' 请求写入 '{}'，不在当前节点 expectedFiles 授权范围内。",
                    invocation.name, path
                )));
            }
            return Ok(json!({
                "mode": "expected_files",
                "decision": "allow",
                "path": path,
            }));
        }

        if spec.access.mutates_filesystem {
            if matches!(invocation.name.as_str(), "exec" | "local_zsh") {
                // Shell 的实际写集不能靠字符串静态推断；它必须继续经过下方
                // ReviewRequired 门禁。文件 API 则已经由 expectedFiles 精确约束。
                return Ok(json!({
                    "mode": "command_review",
                    "decision": "defer_to_safety_review",
                    "expectedFiles": self.capabilities.write_scopes_for_review(),
                }));
            }
            return Err(ToolResult::recoverable_error(format!(
                "错误：工具 '{}' 具有文件写副作用，但当前资源授权无法绑定其目标路径，已按 fail-closed 拒绝。",
                invocation.name
            )));
        }

        Ok(json!({ "mode": "read_or_external", "decision": "allow" }))
    }

    async fn start_child_run(
        &self,
        invocation: &CapabilityInvocation,
    ) -> anyhow::Result<Option<String>> {
        let Some(audit) = self.audit else {
            return Ok(None);
        };
        let tool_call = RequestedToolCall {
            id: invocation.id.clone(),
            name: invocation.name.clone(),
            arguments: invocation.arguments.clone(),
        };
        ToolRuntime::create_and_start_tool_run_with_trace(
            audit.db,
            self.registry,
            audit.workspace_id,
            self.workspace,
            audit.on_event,
            &tool_call,
            ToolRunTraceContext {
                parent_run_id: Some(audit.parent_run_id.to_string()),
                origin: invocation.origin.as_str().to_string(),
                step_id: invocation.step_id.clone(),
                sequence: invocation.sequence,
            },
        )
        .await
        .map(Some)
    }

    async fn finish_child_run(
        &self,
        run_id: Option<String>,
        invocation: &CapabilityInvocation,
        mut result: ToolResult,
    ) -> ToolResult {
        let (Some(audit), Some(run_id)) = (self.audit, run_id) else {
            return result;
        };
        if !result.artifacts.is_empty() {
            if let Err(error) = audit
                .db
                .insert_tool_artifacts_for_run_async(
                    audit.workspace_id,
                    &run_id,
                    &invocation.id,
                    &invocation.name,
                    &result.artifacts,
                )
                .await
            {
                result = ToolResult::fatal_error(format!(
                    "错误：内部工具 '{}' 已执行，但产物审计失败：{error:#}",
                    invocation.name
                ));
            }
        }
        let result_text = result.output_for_llm();
        let metadata = result.run_metadata_json();
        if let Err(error) = ToolRuntime::finish_tool_run(
            audit.db,
            audit.on_event,
            &run_id,
            ToolRunFinishUpdate {
                status: result.status.as_run_status(),
                result_mode: Some("raw"),
                message_id: None,
                error_kind: result.status.error_kind(),
                error_message: result.status.error_kind().map(|_| result_text.as_str()),
                action_kind: result.action.as_ref().map(super::ToolAction::kind),
                metadata_json: metadata.as_deref(),
            },
        )
        .await
        {
            return ToolResult::fatal_error(format!(
                "错误：内部工具 '{}' 已执行，但审计记录收尾失败：{error:#}",
                invocation.name
            ));
        }
        result
    }

    async fn await_without_timeout<F>(
        &self,
        invocation: &CapabilityInvocation,
        cancellable: bool,
        execution_cancel_tx: &watch::Sender<bool>,
        mut execution: std::pin::Pin<&mut F>,
    ) -> ToolResult
    where
        F: std::future::Future<Output = ToolResult>,
    {
        let Some(mut cancel_rx) = self.cancel_rx.clone().filter(|_| cancellable) else {
            return execution.await;
        };
        if *cancel_rx.borrow() {
            return cancelled_before_start(invocation);
        }

        tokio::select! {
            result = execution.as_mut() => result,
            changed = cancel_rx.changed() => {
                match changed {
                    Ok(()) if *cancel_rx.borrow() => {
                        cancel_and_settle(invocation, execution_cancel_tx, execution.as_mut()).await
                    }
                    Ok(()) => execution.await,
                    Err(_) => {
                        cancel_and_settle(invocation, execution_cancel_tx, execution.as_mut()).await
                    }
                }
            }
        }
    }

    async fn await_with_timeout<F>(
        &self,
        invocation: &CapabilityInvocation,
        cancellable: bool,
        timeout: Duration,
        execution_cancel_tx: &watch::Sender<bool>,
        mut execution: std::pin::Pin<&mut F>,
    ) -> ToolResult
    where
        F: std::future::Future<Output = ToolResult>,
    {
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);

        let Some(mut cancel_rx) = self.cancel_rx.clone().filter(|_| cancellable) else {
            return tokio::select! {
                biased;
                result = execution.as_mut() => result,
                _ = &mut deadline => cancel_and_settle_timeout(
                    invocation,
                    timeout,
                    execution_cancel_tx,
                    execution.as_mut(),
                ).await,
            };
        };
        if *cancel_rx.borrow() {
            return cancelled_before_start(invocation);
        }

        tokio::select! {
            biased;
            result = execution.as_mut() => result,
            _ = &mut deadline => cancel_and_settle_timeout(
                invocation,
                timeout,
                execution_cancel_tx,
                execution.as_mut(),
            ).await,
            changed = cancel_rx.changed() => {
                match changed {
                    Ok(()) if *cancel_rx.borrow() => {
                        cancel_and_settle(invocation, execution_cancel_tx, execution.as_mut()).await
                    }
                    Ok(()) => tokio::select! {
                        biased;
                        result = execution.as_mut() => result,
                        _ = &mut deadline => cancel_and_settle_timeout(
                            invocation,
                            timeout,
                            execution_cancel_tx,
                            execution.as_mut(),
                        ).await,
                    },
                    Err(_) => {
                        cancel_and_settle(invocation, execution_cancel_tx, execution.as_mut()).await
                    }
                }
            }
        }
    }
}

async fn cancel_and_settle<F>(
    invocation: &CapabilityInvocation,
    execution_cancel_tx: &watch::Sender<bool>,
    mut execution: std::pin::Pin<&mut F>,
) -> ToolResult
where
    F: std::future::Future<Output = ToolResult>,
{
    let _ = execution_cancel_tx.send(true);
    match tokio::time::timeout(CANCELLATION_SETTLE_GRACE, execution.as_mut()).await {
        Ok(result) => settled_after_cancellation(result),
        Err(_) => cancellation_unsettled(invocation),
    }
}

async fn cancel_and_settle_timeout<F>(
    invocation: &CapabilityInvocation,
    timeout: Duration,
    execution_cancel_tx: &watch::Sender<bool>,
    mut execution: std::pin::Pin<&mut F>,
) -> ToolResult
where
    F: std::future::Future<Output = ToolResult>,
{
    let _ = execution_cancel_tx.send(true);
    match tokio::time::timeout(CANCELLATION_SETTLE_GRACE, execution.as_mut()).await {
        Ok(result) => settled_after_timeout(invocation, timeout, result),
        Err(_) => timeout_result(invocation, timeout),
    }
}

fn timeout_result(invocation: &CapabilityInvocation, timeout: Duration) -> ToolResult {
    with_termination_metadata(
        ToolResult::recoverable_error(format!(
            "错误：工具 '{}' 执行超过统一策略超时 {} 秒，已中止等待；底层阻塞任务可能仍在运行。",
            invocation.name,
            timeout.as_secs()
        )),
        "termination_unknown",
        true,
    )
}

fn settled_after_timeout(
    invocation: &CapabilityInvocation,
    timeout: Duration,
    result: ToolResult,
) -> ToolResult {
    let original_status = result.status.as_run_status();
    let termination_state = if result.status == super::ToolStatus::Cancelled {
        "terminated"
    } else {
        "completed_after_deadline"
    };
    let message = format!(
        "错误：工具 '{}' 执行超过统一策略超时 {} 秒；取消请求后底层已收敛，原始状态为 {}。",
        invocation.name,
        timeout.as_secs(),
        original_status
    );
    let timeout_result = if result.status == super::ToolStatus::FatalError {
        ToolResult::fatal_error(message)
    } else {
        ToolResult::recoverable_error(message)
    };
    let mut timeout_result = with_termination_metadata(timeout_result, termination_state, true);
    if let Value::Object(metadata) = &mut timeout_result.metadata {
        metadata.insert(
            "deadline".to_string(),
            json!({
                "timeoutSeconds": timeout.as_secs(),
                "originalStatus": original_status,
            }),
        );
    }
    timeout_result
}

fn cancelled_before_start(invocation: &CapabilityInvocation) -> ToolResult {
    with_termination_metadata(
        ToolResult::cancelled(format!("工具 '{}' 在开始执行前已取消。", invocation.name)),
        "not_started",
        true,
    )
}

fn cancellation_unsettled(invocation: &CapabilityInvocation) -> ToolResult {
    with_termination_metadata(
        ToolResult::cancelled(format!(
            "错误：工具 '{}' 的等待已取消；底层阻塞任务可能仍在运行。",
            invocation.name
        )),
        "termination_unknown",
        true,
    )
}

fn settled_after_cancellation(result: ToolResult) -> ToolResult {
    let state = if result.status == super::ToolStatus::Cancelled {
        "terminated"
    } else {
        // 调用在取消请求后自行完成；保留真实成功/失败状态，避免把已经发生的
        // 副作用伪装成“已取消”。外层 run loop 仍会在本调用落库后停止。
        "completed_after_cancel_request"
    };
    with_termination_metadata(result, state, true)
}

fn with_termination_metadata(
    mut result: ToolResult,
    termination_state: &str,
    cancel_requested: bool,
) -> ToolResult {
    let termination = json!({
        "cancelRequested": cancel_requested,
        "state": termination_state,
        "confirmed": termination_state != "termination_unknown",
    });
    match &mut result.metadata {
        Value::Object(metadata) => {
            metadata.insert("termination".to_string(), termination);
        }
        Value::Null => result.metadata = json!({ "termination": termination }),
        other => {
            result.metadata = json!({
                "value": mem::take(other),
                "termination": termination,
            });
        }
    }
    result
}

fn with_broker_metadata(mut result: ToolResult, invocation: &CapabilityInvocation) -> ToolResult {
    let broker = json!({
        "invocationId": invocation.id,
        "origin": invocation.origin.as_str(),
        "stepId": invocation.step_id,
        "sequence": invocation.sequence,
    });
    match &mut result.metadata {
        Value::Object(metadata) => {
            metadata.insert("broker".to_string(), broker);
        }
        Value::Null => result.metadata = json!({ "broker": broker }),
        other => {
            result.metadata = json!({
                "value": std::mem::take(other),
                "broker": broker,
            });
        }
    }
    result
}

fn with_policy_metadata(mut result: ToolResult, policy: Value) -> ToolResult {
    match &mut result.metadata {
        Value::Object(metadata) => {
            metadata.insert("policyDecision".to_string(), policy);
        }
        Value::Null => result.metadata = json!({ "policyDecision": policy }),
        other => {
            result.metadata = json!({
                "value": mem::take(other),
                "policyDecision": policy,
            });
        }
    }
    result
}

fn enforce_tool_program_result_budget(
    mut result: ToolResult,
    invocation: &CapabilityInvocation,
    raw_audited: bool,
) -> ToolResult {
    if result.status != super::ToolStatus::Success {
        return result;
    }
    let data_bytes = result
        .data
        .as_ref()
        .map(|data| serialized_json_size(data).unwrap_or(usize::MAX))
        .unwrap_or(0);
    let output_bytes = serialized_json_size(result.output_for_llm_ref()).unwrap_or(usize::MAX);
    let metadata_bytes = serialized_json_size(&result.metadata).unwrap_or(usize::MAX);
    let total_bytes = data_bytes
        .saturating_add(output_bytes)
        .saturating_add(metadata_bytes)
        .saturating_add(128);
    if total_bytes <= MAX_TOOL_PROGRAM_RESULT_BYTES {
        return result;
    }

    let artifacts = mem::take(&mut result.artifacts);
    let audit_note = if raw_audited {
        "完整原文已保存为该子调用的审计产物；"
    } else {
        ""
    };
    let mut error = ToolResult::recoverable_error(format!(
        "错误：ToolProgram 步骤 '{}' 调用工具 '{}' 的结构化结果约 {} 字节，超过单步 {} 字节预算；{audit_note}请缩小 paths、limit、max_results 或搜索范围后重试。",
        invocation.step_id.as_deref().unwrap_or("<unknown>"),
        invocation.name,
        total_bytes,
        MAX_TOOL_PROGRAM_RESULT_BYTES,
    ));
    error.artifacts = artifacts;
    error.metadata = json!({
        "broker": {
            "invocationId": invocation.id,
            "origin": invocation.origin.as_str(),
            "stepId": invocation.step_id,
            "sequence": invocation.sequence,
        },
        "resultBudget": {
            "exceeded": true,
            "estimatedBytes": total_bytes,
            "maxBytes": MAX_TOOL_PROGRAM_RESULT_BYTES,
            "dataBytes": data_bytes,
            "outputBytes": output_bytes,
            "metadataBytes": metadata_bytes,
        }
    });
    error
}

fn serialized_json_size<T: Serialize + ?Sized>(value: &T) -> io::Result<usize> {
    #[derive(Default)]
    struct Counter(usize);
    impl io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let mut counter = Counter::default();
    serde_json::to_writer(&mut counter, value).map_err(io::Error::other)?;
    Ok(counter.0)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use async_trait::async_trait;
    use parking_lot::Mutex;
    use serde_json::{json, Value};
    use tokio::sync::watch;

    use super::{CapabilityBroker, CapabilityInvocation};
    use crate::agent::tools::{
        AgentTool, CapabilitySet, ToolContext, ToolRegistry, ToolResult, ToolSafety, ToolSpec,
    };

    struct Echo;
    struct WaitForCancel;
    struct ScopedWrite;

    #[async_trait]
    impl AgentTool for Echo {
        fn name(&self) -> &'static str {
            "echo"
        }

        fn description(&self) -> &'static str {
            "echo"
        }

        fn parameters(&self) -> Value {
            json!({
                "type": "object",
                "properties": { "value": { "type": "string" } },
                "required": ["value"],
                "additionalProperties": false
            })
        }

        fn spec(&self) -> ToolSpec {
            let mut spec = ToolSpec::new(self.name(), self.description(), self.parameters());
            spec.safety = ToolSafety::Safe;
            spec
        }

        async fn execute(&self, args: &Value, _context: &ToolContext) -> ToolResult {
            ToolResult::success_data(
                json!({ "value": args["value"] }),
                args["value"].as_str().unwrap_or_default(),
                args["value"].as_str().unwrap_or_default(),
            )
        }
    }

    #[async_trait]
    impl AgentTool for WaitForCancel {
        fn name(&self) -> &'static str {
            "wait_for_cancel"
        }

        fn description(&self) -> &'static str {
            "wait"
        }

        fn parameters(&self) -> Value {
            json!({ "type": "object", "additionalProperties": false })
        }

        fn spec(&self) -> ToolSpec {
            let mut spec = ToolSpec::new(self.name(), self.description(), self.parameters());
            spec.safety = ToolSafety::Safe;
            spec.execution.timeout_secs = 10;
            spec
        }

        async fn execute(&self, _args: &Value, context: &ToolContext) -> ToolResult {
            let Some(mut cancel_rx) = context.cancel_rx.clone() else {
                return ToolResult::fatal_error("测试工具缺少取消信号");
            };
            while !*cancel_rx.borrow() {
                if cancel_rx.changed().await.is_err() {
                    return ToolResult::fatal_error("取消通道提前关闭");
                }
            }
            ToolResult::cancelled("测试工具已终止")
        }
    }

    #[async_trait]
    impl AgentTool for ScopedWrite {
        fn name(&self) -> &'static str {
            "write_file"
        }

        fn description(&self) -> &'static str {
            "write"
        }

        fn parameters(&self) -> Value {
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" },
                },
                "required": ["path", "content"],
            })
        }

        async fn execute(&self, _args: &Value, _context: &ToolContext) -> ToolResult {
            ToolResult::success_text("written")
        }
    }

    fn context() -> ToolContext {
        ToolContext {
            workspace_id: "ws".to_string(),
            workspace: std::env::current_dir().expect("current dir"),
            session_title: "test".to_string(),
            user_task: None,
            ssh_review: None,
            exec_timeout_secs: 1,
            restrict_to_workspace: true,
            extra_allowed_dirs: Vec::new(),
            app_handle: None,
            llm_provider: None,
            vision_model: String::new(),
            image_model_url: String::new(),
            image_model_api_key: String::new(),
            image_model: String::new(),
            image_edit_model: String::new(),
            sub_agent_tool_registry: None,
            current_sub_agent_id: None,
            current_sub_agent_name: None,
            current_tool_call_id: None,
            current_tool_spec_hash: None,
            cancel_rx: None,
            sub_agent_parent_tool_call_id: None,
            sub_agent_trace_events: Some(std::sync::Arc::new(Mutex::new(Vec::new()))),
        }
    }

    #[tokio::test]
    async fn denies_calls_outside_the_grant_before_dispatch() {
        let registry = ToolRegistry::new(vec![Box::new(Echo)]);
        let context = context();
        let broker = CapabilityBroker::new(
            &registry,
            Path::new(&context.workspace),
            CapabilitySet::default(),
            &context,
        );

        let result = broker
            .invoke(CapabilityInvocation::model(
                "call-1",
                "echo",
                json!({ "value": "hello" }),
            ))
            .await;

        assert!(result.output_for_llm().contains("未授予"));
    }

    #[tokio::test]
    async fn validates_arguments_then_dispatches_with_structured_data() {
        let registry = ToolRegistry::new(vec![Box::new(Echo)]);
        let context = context();
        let broker = CapabilityBroker::new(
            &registry,
            Path::new(&context.workspace),
            CapabilitySet::new(["echo".to_string()]),
            &context,
        );

        let invalid = broker
            .invoke(CapabilityInvocation::model(
                "call-1",
                "echo",
                json!({ "value": 7 }),
            ))
            .await;
        assert!(invalid.output_for_llm().contains("参数"));

        let valid = broker
            .invoke(CapabilityInvocation::model(
                "call-2",
                "echo",
                json!({ "value": "hello" }),
            ))
            .await;
        assert_eq!(valid.data, Some(json!({ "value": "hello" })));
        assert_eq!(valid.metadata["broker"]["origin"], "model");
    }

    #[tokio::test]
    async fn rejects_oversized_tool_program_result_before_building_envelope() {
        let registry = ToolRegistry::new(vec![Box::new(Echo)]);
        let context = context();
        let broker = CapabilityBroker::new(
            &registry,
            Path::new(&context.workspace),
            CapabilitySet::new(["echo".to_string()]),
            &context,
        );

        let result = broker
            .invoke(CapabilityInvocation::tool_program(
                "outer:1",
                "large",
                1,
                "echo",
                json!({ "value": "x".repeat(200 * 1024) }),
            ))
            .await;

        assert_eq!(
            result.status,
            crate::agent::tools::ToolStatus::RecoverableError
        );
        assert!(result.output_for_llm().contains("超过单步"));
        assert_eq!(result.metadata["resultBudget"]["exceeded"], true);
        assert!(result.data.is_none());
    }

    #[tokio::test]
    async fn records_confirmed_cooperative_cancellation() {
        let registry = ToolRegistry::new(vec![Box::new(WaitForCancel)]);
        let context = context();
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let broker = CapabilityBroker::new(
            &registry,
            Path::new(&context.workspace),
            CapabilitySet::new(["wait_for_cancel".to_string()]),
            &context,
        )
        .with_cancellation(cancel_rx);

        let invocation = broker.invoke(CapabilityInvocation::model(
            "call-cancel",
            "wait_for_cancel",
            json!({}),
        ));
        let request_cancel = async move {
            tokio::task::yield_now().await;
            cancel_tx.send(true).expect("send cancellation");
        };
        let (result, ()) = tokio::join!(invocation, request_cancel);

        assert_eq!(result.status, crate::agent::tools::ToolStatus::Cancelled);
        assert_eq!(result.metadata["termination"]["state"], "terminated");
        assert_eq!(result.metadata["termination"]["confirmed"], true);
    }

    #[tokio::test]
    async fn unified_timeout_requests_tool_cancellation_and_waits_for_settlement() {
        let registry = ToolRegistry::new(vec![]);
        let context = context();
        let broker = CapabilityBroker::new(
            &registry,
            Path::new(&context.workspace),
            CapabilitySet::default(),
            &context,
        );
        let invocation = CapabilityInvocation::model("deadline", "slow_tool", json!({}));
        let (execution_cancel_tx, mut execution_cancel_rx) = watch::channel(false);
        let execution = async move {
            while !*execution_cancel_rx.borrow() {
                execution_cancel_rx
                    .changed()
                    .await
                    .expect("cancellation channel remains open");
            }
            ToolResult::cancelled("cooperatively stopped")
        };
        tokio::pin!(execution);

        let result = broker
            .await_with_timeout(
                &invocation,
                true,
                Duration::from_millis(10),
                &execution_cancel_tx,
                execution,
            )
            .await;

        assert_eq!(
            result.status,
            crate::agent::tools::ToolStatus::RecoverableError
        );
        assert_eq!(result.metadata["termination"]["state"], "terminated");
        assert_eq!(result.metadata["termination"]["confirmed"], true);
        assert_eq!(result.metadata["deadline"]["originalStatus"], "cancelled");
    }

    #[tokio::test]
    async fn enforces_graph_write_scopes_before_dispatch() {
        let registry = ToolRegistry::new(vec![Box::new(ScopedWrite)]);
        let context = context();
        let broker = CapabilityBroker::new(
            &registry,
            Path::new(&context.workspace),
            CapabilitySet::new(["write_file".to_string()])
                .restrict_writes_to(["src/allowed.rs".to_string()]),
            &context,
        );

        let denied = broker
            .invoke(CapabilityInvocation::model(
                "write-denied",
                "write_file",
                json!({ "path": "src/other.rs", "content": "x" }),
            ))
            .await;
        let allowed = broker
            .invoke(CapabilityInvocation::model(
                "write-allowed",
                "write_file",
                json!({ "path": "src/allowed.rs", "content": "x" }),
            ))
            .await;

        assert_eq!(
            denied.status,
            crate::agent::tools::ToolStatus::RecoverableError
        );
        assert!(denied.output_for_llm().contains("expectedFiles"));
        assert_eq!(allowed.status, crate::agent::tools::ToolStatus::Success);
        assert_eq!(
            allowed.metadata["policyDecision"]["resourceScope"]["mode"],
            "expected_files"
        );
    }
}

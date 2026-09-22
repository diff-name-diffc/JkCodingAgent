//! 应用级工具执行策略：审查门禁 + 参数准备 + 台账 + 统一超时。
//!
//! 迁移自旧 `CapabilityBroker` 的策略层（`tools/broker.rs` 的 authorize/
//! 参数准备/执行包装与 `tools/runtime.rs` 的台账）。设计上不再有「能力仲裁」
//! 与「spec hash 注入」——rig 工具面本身即授权集（工具面在组装期按允许列表
//! 收敛），台账元数据由工具名对应的 `ToolSpec` 策略表派生。
//!
//! 门禁顺序（对齐旧 broker）：
//! 1. 台账创建 + started（无论后续是否被拒绝，先落痕迹，避免审计缺口）；
//! 2. 取消检查；
//! 3. 参数准备（schema 默认值注入 + Draft 2020-12 校验）；
//! 4. `ToolSafety::Dangerous` 直接拒绝；
//! 5. `ReviewRequired && !review_self_managed` 走通用审查（未配置审查
//!    fail-closed 拒绝）；自管审查的工具（local_zsh / ssh_exec /
//!    sync_directory / MCP 桥）在工具内部完成审查，此处放行。

use std::path::PathBuf;
use std::time::Duration;

use rig::message::ToolCall;
use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use tauri::ipc::Channel;
use tokio::sync::watch;

use super::surface::{ToolCallGuard, ToolCallOutcome, ToolCallTrace, ToolExecutionPolicy};
use crate::agent::common::cancellation_requested;
use crate::agent::db::{DispatcherDb, ToolRunTraceContext};
use crate::agent::rig_ext::review::RigReviewContext;
use crate::agent::rig_ext::tools::run_record::{
    finish_tool_run, prepare_arguments, start_tool_run, RigToolRun, RigToolRunContext,
    RigToolRunFinish,
};
use crate::agent::run_loop::AgentEvent;
use crate::agent::tools::spec::{ToolSafety, ToolSpec};

/// MCP 动态工具的 canonical 名前缀（见 `mcp/registry.rs`）。
const MCP_TOOL_NAME_PREFIX: &str = "mcp__";

/// 应用级策略的构造输入。
pub struct AppToolPolicyConfig {
    pub workspace_id: String,
    pub workspace: PathBuf,
    /// 非自管审查工具的通用审查输入（`ReviewRequired` 且非自管者）。
    pub review: RigReviewContext,
    /// run 级取消信号（门禁取消检查用；工具自身取消经 `RigToolDeps.cancel_rx`）。
    pub cancel_rx: Option<watch::Receiver<bool>>,
    /// 子智能体工具调用的台账 trace 上下文（根 Agent 为默认值）。
    pub trace: ToolRunTraceContext,
}

/// 应用级执行策略：借用 DB 与事件通道，持有门禁输入。
pub struct AppToolExecutionPolicy<'a> {
    db: &'a DispatcherDb,
    on_event: &'a Channel<AgentEvent>,
    config: AppToolPolicyConfig,
}

impl<'a> AppToolExecutionPolicy<'a> {
    pub fn new(
        db: &'a DispatcherDb,
        on_event: &'a Channel<AgentEvent>,
        config: AppToolPolicyConfig,
    ) -> Self {
        Self {
            db,
            on_event,
            config,
        }
    }

    /// 工具名 → 策略规格：MCP 工具用 `ToolSpec::mcp`（自管审查 + 网络外部效应），
    /// 其余按策略表查（未收录的名字由 `ToolSpec::new` 走 fail-closed 兜底）。
    fn spec_for(&self, tool: &PortableDynamicTool) -> (ToolSpec, bool) {
        let name = tool.name();
        if name.starts_with(MCP_TOOL_NAME_PREFIX) {
            let definition = tool.definition();
            return (
                ToolSpec::mcp(name.to_string(), definition.description, definition.parameters),
                true,
            );
        }
        let definition = tool.definition();
        let registered = crate::agent::tools::spec::is_registered_tool_name(name);
        (
            ToolSpec::new(name, &definition.description, definition.parameters),
            registered,
        )
    }
}

#[async_trait::async_trait]
impl ToolExecutionPolicy for AppToolExecutionPolicy<'_> {
    async fn before_call(&self, tool: &PortableDynamicTool, call: &ToolCall) -> ToolCallGuard {
        let (spec, registered) = self.spec_for(tool);
        let tool_call_id = call.wire_call_id().to_string();
        let run_context = RigToolRunContext {
            db: self.db,
            workspace_id: &self.config.workspace_id,
            on_event: self.on_event,
        };

        // 1. 台账创建 + started：无效参数同样先进入台账（旧实现口径）。
        let effective_arguments = prepare_arguments(
            &spec.name,
            &spec.parameters,
            &call.function.arguments,
        )
        .unwrap_or_else(|_| call.function.arguments.clone());
        let trace = match start_tool_run(
            run_context,
            &spec,
            registered,
            &tool_call_id,
            &call.function.arguments,
            &effective_arguments,
            self.config.trace.clone(),
        )
        .await
        {
            Ok(run) => Some(ToolCallTrace {
                run_id: Some(run.run_id),
            }),
            Err(error) => {
                eprintln!(
                    "错误：创建工具运行记录失败（工具 {}）：{error}",
                    spec.name
                );
                None
            }
        };

        let reject_with = |error: ToolExecutionError| ToolCallGuard {
            trace: trace.clone(),
            rejection: Some(error),
        };

        // 2. 取消检查：run 级取消优先，尚未执行即收口。
        if self
            .config
            .cancel_rx
            .as_ref()
            .is_some_and(cancellation_requested)
        {
            return reject_with(ToolExecutionError::cancelled(format!(
                "错误：工具 '{}' 尚未执行，本轮运行已取消。",
                spec.name
            )));
        }

        // 3. 参数准备：schema 默认值注入 + 校验（失败回灌可恢复错误）。
        if let Err(error) =
            prepare_arguments(&spec.name, &spec.parameters, &call.function.arguments)
        {
            return reject_with(
                ToolExecutionError::other(error.message).with_code(error.code.to_string()),
            );
        }

        // 4. 策略标记为 dangerous 的工具：运行时默认拒绝。
        if spec.safety == ToolSafety::Dangerous {
            return reject_with(ToolExecutionError::other(format!(
                "错误：工具 '{}' 被策略标记为 dangerous，运行时默认拒绝执行。",
                spec.name
            )));
        }

        // 5. ReviewRequired 且非自管审查：通用审查（未配置审查 fail-closed）。
        if spec.safety == ToolSafety::ReviewRequired && !spec.review_self_managed {
            if let Err(rejection) = self.generic_review(&spec, call).await {
                return reject_with(rejection);
            }
        }

        ToolCallGuard {
            trace,
            rejection: None,
        }
    }

    async fn execute(
        &self,
        tool: &PortableDynamicTool,
        call: &ToolCall,
    ) -> Result<ToolOutput, ToolExecutionError> {
        let (spec, _) = self.spec_for(tool);
        let arguments = call.function.arguments.clone();

        // 统一超时：`unified_timeout=false`（自管超时）或 timeout_secs=0 时跳过，
        // 由工具自管生命周期（对齐旧 ToolExecutionPolicy 语义）。
        if !spec.execution.unified_timeout || spec.execution.timeout_secs == 0 {
            return tool.execute(arguments).await;
        }
        let timeout = Duration::from_secs(spec.execution.timeout_secs);
        match tokio::time::timeout(timeout, tool.execute(arguments)).await {
            Ok(result) => result,
            Err(_) => Err(ToolExecutionError::timeout(format!(
                "错误：工具 '{}' 执行超时（{}秒），已终止等待。",
                spec.name, spec.execution.timeout_secs
            ))),
        }
    }

    async fn after_call(
        &self,
        trace: Option<&ToolCallTrace>,
        _call: &ToolCall,
        outcome: ToolCallOutcome<'_>,
    ) {
        let Some(run_id) = trace.and_then(|trace| trace.run_id.as_deref()) else {
            return;
        };
        let run = RigToolRun {
            run_id: run_id.to_string(),
        };
        let update = RigToolRunFinish {
            status: outcome.status,
            result_mode: outcome.result_mode,
            message_id: outcome.message_id,
            error_kind: outcome.error_kind,
            error_message: outcome.error_message,
            action_kind: outcome.action_kind,
            metadata_json: None,
        };
        if let Err(error) = finish_tool_run(
            RigToolRunContext {
                db: self.db,
                workspace_id: &self.config.workspace_id,
                on_event: self.on_event,
            },
            &run,
            update,
        )
        .await
        {
            // 结果已落库、台账收尾失败仅告警（对齐旧 finish_tool_run 的告警语义）。
            eprintln!("错误：工具运行记录 {run_id} 收尾失败（结果已持久化）：{error}");
        }
    }
}

impl AppToolExecutionPolicy<'_> {
    /// 通用审查（非自管审查的 `ReviewRequired` 工具）：送审工具名 + 完整参数。
    async fn generic_review(
        &self,
        spec: &ToolSpec,
        call: &ToolCall,
    ) -> Result<(), ToolExecutionError> {
        let Some(review_config) = self.config.review.config.as_ref() else {
            // fail-closed：未配置审查模型即拒绝。
            return Err(ToolExecutionError::refused(format!(
                "错误：工具 '{}' 需要安全审查，但当前执行上下文未配置审查模型，已按 fail-closed 拒绝。",
                spec.name
            )));
        };
        let arguments = serde_json::to_string(&call.function.arguments)
            .unwrap_or_else(|_| call.function.arguments.to_string());
        let payload = self.config.review.build_payload(
            &self.config.workspace_id,
            None,
            crate::agent::ssh_review::CommandReviewTarget::AgentTool {
                workspace_path: self.config.workspace.display().to_string(),
                tool_name: spec.name.clone(),
                provider: spec.provider.clone(),
                policy_summary: format!(
                    "readonly={}, workspaceBound={}, network={}, mutatesFilesystem={}, mutatesExternalState={}",
                    spec.access.readonly,
                    spec.access.workspace_bound,
                    spec.access.requires_network,
                    spec.access.mutates_filesystem,
                    spec.access.mutates_external_state,
                ),
            },
            arguments,
            None,
        );
        let verdict = crate::agent::ssh_review::review_shell_command(review_config, &payload)
            .await
            .map_err(|error| {
                ToolExecutionError::other(format!(
                    "错误：工具 '{}' 安全审查失败，已拒绝执行：{error}",
                    spec.name
                ))
            })?;
        if !verdict.allowed {
            return Err(ToolExecutionError::refused(
                crate::agent::ssh_review::with_confirm_guidance(
                    format!(
                        "错误：工具 '{}' 已被安全审查拦截：{}",
                        spec.name, verdict.reason
                    ),
                    &verdict.reason,
                ),
            ));
        }
        Ok(())
    }
}

/// 无人使用时的空策略（测试/占位路径）：不加门禁、不建台账。
pub fn no_policy() -> DirectPolicy {
    DirectPolicy
}

pub use super::surface::DirectToolExecution as DirectPolicy;

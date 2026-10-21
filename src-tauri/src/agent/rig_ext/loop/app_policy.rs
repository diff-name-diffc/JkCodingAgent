//! 应用级工具执行策略：审查门禁 + 参数准备 + 统一超时。
//!
//! 迁移自旧 `CapabilityBroker` 的策略层（`tools/broker.rs` 的 authorize/
//! 参数准备/执行包装与 `tools/runtime.rs` 的台账）。设计上不再有「能力仲裁」
//! 与「spec hash 注入」——rig 工具面本身即授权集（工具面在组装期按允许列表
//! 收敛）。台账不经本层：登记唯一走调度器 `enqueue` 的准入批次（本层
//! `before_call` 在 `ToolInvocationContext` 作用域内运行，复用准入产物），
//! 终态唯一走 worker 的 `settle_tool_completion`——单写路径，不再有
//! `run_record` 裸路径的 NULL agent_run_id 孤儿行（P0-3）。
//!
//! 门禁顺序（对齐旧 broker，台账步骤已并入调度器准入）：
//! 1. 取消检查；
//! 2. 参数准备（schema 默认值注入 + Draft 2020-12 校验；调度器路径复用
//!    enqueue 准入产出的 effective 值，无上下文回退在此计算一次）；
//! 3. `ToolSafety::Dangerous` 直接拒绝；
//! 4. `ReviewRequired && !review_self_managed` 走通用审查（未配置审查
//!    fail-closed 拒绝）；自管审查的工具（local_zsh / ssh_exec /
//!    sync_directory / MCP 桥）在工具内部完成审查，此处放行。

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

use rig::message::ToolCall;
use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use tauri::ipc::Channel;
use tokio::sync::watch;

use super::scheduler::SETTLE_CEILING;
use super::surface::{ToolCallGuard, ToolExecutionPolicy};
use crate::agent::common::{cancellation_requested, wait_for_optional_cancellation};
use crate::agent::db::{DispatcherDb, ToolRunTraceContext};
use crate::agent::rig_ext::events::AgentEvent;
use crate::agent::rig_ext::review::RigReviewContext;
use crate::agent::rig_ext::tools::common::u64_arg;
use crate::agent::rig_ext::tools::deps::ToolTimeoutDefaults;
use crate::agent::rig_ext::tools::run_record::prepare_arguments;
use crate::agent::rig_ext::tools::spec::{effective_timeout_secs, ToolSafety, ToolSpec};
use crate::mcp::registry::MCP_TOOL_NAME_PREFIX;

/// 应用级策略的构造输入。
#[derive(Clone)]
pub struct AppToolPolicyConfig {
    pub workspace_id: String,
    pub workspace: PathBuf,
    /// 工作区之外可访问的本地根；资源声明必须覆盖它们，避免跨会话写入冲突。
    pub filesystem_roots: Vec<PathBuf>,
    /// 非自管审查工具的通用审查输入（`ReviewRequired` 且非自管者）。
    pub review: RigReviewContext,
    /// run 级取消信号（门禁取消检查用；工具自身取消经 `RigToolDeps.cancel_rx`）。
    pub cancel_rx: Option<watch::Receiver<bool>>,
    /// 子智能体工具调用的台账 trace 上下文（根 Agent 为默认值）。
    pub trace: ToolRunTraceContext,
    /// 用户配置的工具超时默认（AhaSettingsV2.toolTimeouts 解析产物）：
    /// 统一超时白名单工具的 deadline 经 `spec::effective_timeout_secs`
    /// 参与「调用声明 > 用户默认 > 表默认」解析；空 = 全部用表默认。
    pub tool_timeouts: ToolTimeoutDefaults,
    /// 通用「需用户确认」弹窗门禁的 UI 句柄（None = 无 UI，确认一律按拒绝）。
    pub app_handle: Option<tauri::AppHandle>,
}

/// 应用级执行策略：门禁输入；策略层不再持有 DB / 事件通道——台账单写路径
/// （调度器 enqueue 登记 + settle 收口，P0-3）后无策略侧台账消费者。
/// `new` 保留 db / on_event 参数以兼容各装配点签名（合并节点可统一收敛）。
#[derive(Clone)]
pub struct AppToolExecutionPolicy {
    config: AppToolPolicyConfig,
}

impl AppToolExecutionPolicy {
    pub fn new(
        _db: &DispatcherDb,
        _on_event: &Channel<AgentEvent>,
        config: AppToolPolicyConfig,
    ) -> Self {
        Self { config }
    }

    /// 工具名 → 策略规格：MCP 工具用 `ToolSpec::mcp`（自管审查 + 网络外部效应），
    /// 其余按策略表查（未收录的名字由 `ToolSpec::new` 走 fail-closed 兜底）。
    fn spec_for(&self, tool: &PortableDynamicTool) -> ToolSpec {
        let name = tool.name();
        if name.starts_with(MCP_TOOL_NAME_PREFIX) {
            let definition = tool.definition();
            return ToolSpec::mcp(
                name.to_string(),
                definition.description,
                definition.parameters,
            );
        }
        let definition = tool.definition();
        ToolSpec::new(name, &definition.description, definition.parameters)
    }
}

#[async_trait::async_trait]
impl ToolExecutionPolicy for AppToolExecutionPolicy {
    fn registration_trace(&self) -> ToolRunTraceContext {
        self.config.trace.clone()
    }
    fn resource_workspace(&self) -> Option<PathBuf> {
        Some(self.config.workspace.clone())
    }

    fn filesystem_roots(&self) -> &[PathBuf] {
        &self.config.filesystem_roots
    }

    async fn before_call(&self, tool: &PortableDynamicTool, call: &ToolCall) -> ToolCallGuard {
        let spec = self.spec_for(tool);
        let reject_with = |error: ToolExecutionError| ToolCallGuard {
            rejection: Some(error),
        };

        // 1. 取消检查：run 级取消优先，尚未执行即收口。
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

        // 2. 参数准备：schema 默认值注入 + 校验（失败回灌可恢复错误）。
        //    调度器路径的 prepared 来自 enqueue 准入（已用同一纯函数对同一
        //    不可变输入校验）直接放行；无 `ToolInvocationContext` 的调用在此
        //    回退计算一次。
        let prepared = match super::invocation::ToolInvocationContext::current()
            .as_ref()
            .and_then(|context| context.prepared_arguments.clone())
        {
            Some(effective) => Ok(effective),
            None => prepare_arguments(&spec.name, &spec.parameters, &call.function.arguments),
        };
        if let Err(error) = prepared {
            return reject_with(
                ToolExecutionError::other(error.message).with_code(error.code.to_string()),
            );
        }

        // 3. 策略标记为 dangerous 的工具：运行时默认拒绝。
        if spec.safety == ToolSafety::Dangerous {
            return reject_with(ToolExecutionError::other(format!(
                "错误：工具 '{}' 被策略标记为 dangerous，运行时默认拒绝执行。",
                spec.name
            )));
        }

        // 4. ReviewRequired 且非自管审查：通用审查（未配置审查 fail-closed）。
        if spec.safety == ToolSafety::ReviewRequired && !spec.review_self_managed {
            if let Err(rejection) = self.generic_review(&spec, call).await {
                return reject_with(rejection);
            }
        }

        ToolCallGuard { rejection: None }
    }

    async fn execute(
        &self,
        tool: &PortableDynamicTool,
        call: &ToolCall,
    ) -> Result<ToolOutput, ToolExecutionError> {
        let spec = self.spec_for(tool);
        let invocation = super::invocation::ToolInvocationContext::current();
        // 调度器路径复用 enqueue 准入产出的 effective 值（默认注入 + 已校验）；
        // 裸路径（顺序批/无上下文）回退计算一次。
        let arguments = match invocation
            .as_ref()
            .and_then(|context| context.prepared_arguments.clone())
        {
            Some(prepared) => prepared,
            None => prepare_arguments(&spec.name, &spec.parameters, &call.function.arguments)
                .map_err(|error| ToolExecutionError::invalid_args(error.message))?,
        };

        // 等待上限：统一超时工具用有效超时（白名单工具为「调用声明 > 用户配置
        // 默认 > 表默认」，经 spec::effective_timeout_secs 夹紧；白名单外恒为
        // 表值）；自管工具（unified_timeout=false）用兜底上限 `settle_ceiling_secs`
        // （工具自身预算之外的最后防线，取值 = 最坏合法预算），策略层不夹紧其单次值。
        // 两者共用同一套「到点 → 发取消 → 宽限收敛 → 交接后台」流程，差别只在文案与
        // 「是否值得发取消」：统一超时工具恒发，自管工具按 `cancellable` 声明。
        // `timeout_secs == 0` 维持文档语义（不设统一超时限制）；策略表内工具恒 > 0。
        let ceiling_mode = !spec.execution.unified_timeout;
        let deadline_secs = if ceiling_mode {
            spec.execution.settle_ceiling_secs
        } else {
            // 声明值来自经 schema 校验的 effective 参数（minimum/maximum 在
            // 准入期已拒绝越界声明，这里再夹紧一次作纵深防御）。提取走
            // `common::u64_arg`（与工具侧同出口）：JSON Schema 2020-12 放行
            // 整值浮点（40.0），提取层不接受会把声明静默丢弃。
            let declared = u64_arg(&arguments, "timeout_secs");
            effective_timeout_secs(
                &spec.name,
                declared,
                self.config.tool_timeouts.for_tool(&spec.name),
            )
        };
        if deadline_secs == 0 {
            return tool.execute(arguments).await;
        }
        let deadline = Duration::from_secs(deadline_secs);
        let upstream = invocation
            .as_ref()
            .map(|context| context.cancel_rx.clone())
            .or_else(|| self.config.cancel_rx.clone());
        let (cancel, cancel_rx) = watch::channel(false);
        // 在途执行需要能被移交（上限到达时交给后台继续跑），因此持有 owned 工具：
        // `PortableDynamicTool::execute` 借 `&self`，借用无法移进 spawn 后的任务。
        let owned_tool = tool.clone();
        let execution = async move {
            if let Some(mut context) = invocation {
                context.cancel_rx = cancel_rx;
                context.scope(owned_tool.execute(arguments)).await
            } else {
                owned_tool.execute(arguments).await
            }
        };
        let mut execution = Box::pin(execution);
        tokio::select! {
            biased;
            result = &mut execution => result,
            _ = tokio::time::sleep(deadline) => {
                if !ceiling_mode || spec.execution.cancellable {
                    cancel.send_replace(true);
                }
                let Some(settled) = settle_in_flight(execution).await else {
                    return Err(if ceiling_mode {
                        ToolExecutionError::timeout(format!(
                            "错误：工具 '{}' 自管预算上限（{}秒）已过，底层操作未确认收敛，已在后台继续等待其结算：本轮按「结算未确认」处理。",
                            spec.name, deadline_secs
                        ))
                    } else {
                        ToolExecutionError::timeout(format!(
                            "错误：工具 '{}' 执行超时（{}秒），发出取消信号后 {} 秒内底层操作仍未收敛，已在后台继续等待其结算：本轮按「结算未确认」处理。",
                            spec.name, deadline_secs, SETTLE_CEILING.as_secs()
                        ))
                    }
                    .with_retryable(false));
                };
                if let Err(error) = settled {
                    if matches!(error.code(), Some("external_state_unknown" | "fatal")) {
                        return Err(error);
                    }
                }
                Err(if ceiling_mode {
                    ToolExecutionError::timeout(format!(
                        "错误：工具 '{}' 自管预算上限（{}秒）已过，底层操作已按取消收敛。",
                        spec.name, deadline_secs
                    ))
                } else {
                    ToolExecutionError::timeout(format!(
                        "错误：工具 '{}' 执行超时（{}秒），底层操作已收敛。",
                        spec.name, deadline_secs
                    ))
                }
                .with_retryable(false))
            }
            _ = wait_for_optional_cancellation(upstream) => {
                cancel.send_replace(true);
                let Some(settled) = settle_in_flight(execution).await else {
                    return Err(ToolExecutionError::cancelled(format!(
                        "错误：工具 '{}' 执行取消，发出取消信号后 {} 秒内底层操作仍未收敛，已在后台继续等待其结算：本轮按「结算未确认」处理。",
                        spec.name, SETTLE_CEILING.as_secs()
                    )));
                };
                match settled {
                    Err(error) => Err(error),
                    Ok(output) => Err(ToolExecutionError::cancelled(format!(
                        "错误：取消期间底层操作已结算，操作结果：{}",
                        super::support::tool_output_text(&output)
                    ))),
                }
            }
        }
    }
}

/// 等待在途执行收敛：超过 `SETTLE_CEILING` 仍不收敛时，把执行移交后台任务继续跑完
/// （不 drop、不中止在途 I/O，结果丢弃仅留日志），返回 `None` 表示本轮结算未确认。
async fn settle_in_flight<F>(execution: Pin<Box<F>>) -> Option<F::Output>
where
    F: Future + Send + 'static,
{
    let mut execution = execution;
    match tokio::time::timeout(SETTLE_CEILING, &mut execution).await {
        Ok(output) => Some(output),
        Err(_) => {
            tokio::spawn(async move {
                let _ = execution.await;
            });
            None
        }
    }
}

impl AppToolExecutionPolicy {
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
        // 弹窗硬排除（灾难模式）与送审共用同一份参数文本。
        let arguments_text = arguments.clone();
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
            // 需用户确认类拦截：统一走通用弹窗门禁，用户明确允许则放行；
            // 拒绝/超时/取消按 fail-closed 走原拦截。
            let approved_by_user = crate::agent::rig_ext::review_confirm::needs_user_confirmation(
                &verdict.reason,
                &arguments_text,
                false,
            ) && matches!(
                crate::agent::rig_ext::review_confirm::request_confirmation(
                    self.config.app_handle.as_ref(),
                    self.config.cancel_rx.clone(),
                    crate::agent::rig_ext::review_confirm::ConfirmRequest {
                        workspace_id: self.config.workspace_id.clone(),
                        tool: spec.name.clone(),
                        target: format!("工具 '{}'", spec.name),
                        command: call.function.arguments.to_string(),
                        reason: verdict.reason.clone(),
                        elevated: false,
                    },
                )
                .await,
                crate::agent::rig_ext::review_confirm::ConfirmOutcome::Approved
            );
            if !approved_by_user {
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
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

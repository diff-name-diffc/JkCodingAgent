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
mod tests {
    use super::*;
    use rig::message::ToolFunction;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// 在途执行的 drop 计数：上限到达后必须仍为 0（未丢弃）。
    struct DropFlag(Arc<AtomicUsize>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn policy_with(
        tool_timeouts: ToolTimeoutDefaults,
    ) -> (AppToolExecutionPolicy, crate::test_util::TempDirGuard) {
        let dir = crate::test_util::TempDirGuard::new("rig-app-policy");
        let db = DispatcherDb::new(dir.path().join("jkbot.sqlite3")).expect("open temp db");
        let events = Channel::new(|_| Ok(()));
        let policy = AppToolExecutionPolicy::new(
            &db,
            &events,
            AppToolPolicyConfig {
                workspace_id: "workspace".into(),
                workspace: dir.path().to_path_buf(),
                review: RigReviewContext::unconfigured(),
                cancel_rx: None,
                trace: Default::default(),
                tool_timeouts,
                app_handle: None,
            },
        );
        (policy, dir)
    }

    fn policy() -> (AppToolExecutionPolicy, crate::test_util::TempDirGuard) {
        policy_with(ToolTimeoutDefaults::default())
    }

    /// 带用户配置超时默认的策略（deadline 解析测试用）。
    fn policy_with_timeouts(
        tool_timeouts: ToolTimeoutDefaults,
    ) -> (AppToolExecutionPolicy, crate::test_util::TempDirGuard) {
        policy_with(tool_timeouts)
    }

    /// 永不返回的桩工具：deadline 触发后不收敛，错误文案携带实际 deadline
    /// 秒数（「执行超时（N秒）」），据此断言解析结果。
    fn stuck_tool(name: &'static str) -> PortableDynamicTool {
        PortableDynamicTool::new(name, "卡死工具", json!({"type":"object"}), move |_| {
            Box::pin(async move {
                std::future::pending::<Result<ToolOutput, ToolExecutionError>>().await
            })
        })
    }

    /// 白名单工具的调用声明超时优先于策略表默认：声明 40 秒的 generate_image
    /// 必须在 40 秒（而非表默认 120 秒）触发统一超时。
    #[tokio::test(start_paused = true)]
    async fn declared_call_timeout_overrides_table_default() {
        let (policy, _dir) = policy();
        let tool = stuck_tool("generate_image");
        let call = ToolCall::from_wire(
            "call",
            ToolFunction {
                name: "generate_image".into(),
                arguments: json!({ "timeout_secs": 40 }),
            },
        );
        let task = tokio::spawn(async move { policy.execute(&tool, &call).await });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(40)).await;
        tokio::time::advance(SETTLE_CEILING + Duration::from_secs(1)).await;
        let error = task.await.expect("join").expect_err("必须返回错误");
        assert!(
            error.message().contains("执行超时（40秒）"),
            "声明 40 秒必须生效，实际错误：{}",
            error.message()
        );
    }

    /// 未声明时用户配置默认优先于表默认：配置 90 秒后 deadline 为 90 秒。
    #[tokio::test(start_paused = true)]
    async fn user_default_timeout_applies_without_declaration() {
        let (policy, _dir) = policy_with_timeouts(ToolTimeoutDefaults {
            generate_image: Some(90),
            ..Default::default()
        });
        let tool = stuck_tool("generate_image");
        let call = ToolCall::from_wire(
            "call",
            ToolFunction {
                name: "generate_image".into(),
                arguments: json!({}),
            },
        );
        let task = tokio::spawn(async move { policy.execute(&tool, &call).await });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(90)).await;
        tokio::time::advance(SETTLE_CEILING + Duration::from_secs(1)).await;
        let error = task.await.expect("join").expect_err("必须返回错误");
        assert!(
            error.message().contains("执行超时（90秒）"),
            "用户默认 90 秒必须生效，实际错误：{}",
            error.message()
        );
    }

    /// 上限只兜住真正卡死的工具：发出取消信号后仍不收敛时，不丢弃在途执行
    /// （移交后台继续收敛），调用方拿到明确的「结算未确认」错误而不是无限等待。
    #[tokio::test(start_paused = true)]
    async fn settle_ceiling_hands_off_stuck_execution_without_dropping_it() {
        let (policy, _dir) = policy();
        let dropped = Arc::new(AtomicUsize::new(0));
        let tool_dropped = dropped.clone();
        // 忽略取消信号且永不返回：模拟取消后仍不收敛的工具。
        let tool = PortableDynamicTool::new(
            "read_file",
            "卡死工具",
            json!({"type":"object"}),
            move |_| {
                let tool_dropped = tool_dropped.clone();
                Box::pin(async move {
                    let _guard = DropFlag(tool_dropped);
                    std::future::pending::<Result<ToolOutput, ToolExecutionError>>().await
                })
            },
        );
        let call = ToolCall::from_wire(
            "call",
            ToolFunction {
                name: "read_file".into(),
                arguments: json!({}),
            },
        );
        let task = tokio::spawn(async move { policy.execute(&tool, &call).await });
        tokio::task::yield_now().await;
        // read_file 统一超时 30 秒 → 发取消信号 → 再等满 SETTLE_CEILING 仍不收敛。
        tokio::time::advance(Duration::from_secs(30)).await;
        tokio::time::advance(SETTLE_CEILING + Duration::from_secs(1)).await;
        let error = task.await.expect("join").expect_err("必须返回错误");
        assert_eq!(error.retryable(), Some(false));
        assert!(
            error.message().contains("结算未确认"),
            "错误应说明结算未确认：{}",
            error.message()
        );
        tokio::task::yield_now().await;
        assert_eq!(
            dropped.load(Ordering::SeqCst),
            0,
            "上限到达不得丢弃仍在收敛的在途执行"
        );
    }

    /// 自管工具（unified_timeout=false）自带预算，但仍受兜底上限约束：
    /// 到上限 → 按 `cancellable` 发取消 → 宽限收敛 → 仍不收则交接后台并按「结算未确认」收口。
    #[tokio::test(start_paused = true)]
    async fn self_managed_tool_hits_settle_ceiling_and_hands_off() {
        let (policy, _dir) = policy();
        let dropped = Arc::new(AtomicUsize::new(0));
        let tool_dropped = dropped.clone();
        // local_zsh：策略表里的自管工具（声明预算 60 秒、兜底上限 600 秒）。
        let tool = PortableDynamicTool::new(
            "local_zsh",
            "卡死的自管工具",
            json!({"type":"object"}),
            move |_| {
                let tool_dropped = tool_dropped.clone();
                Box::pin(async move {
                    let _guard = DropFlag(tool_dropped);
                    std::future::pending::<Result<ToolOutput, ToolExecutionError>>().await
                })
            },
        );
        let call = ToolCall::from_wire(
            "call",
            ToolFunction {
                name: "local_zsh".into(),
                arguments: json!({}),
            },
        );
        let task = tokio::spawn(async move { policy.execute(&tool, &call).await });
        tokio::task::yield_now().await;
        // 兜底上限 600 秒：未到点前不得被判失败。
        tokio::time::advance(Duration::from_secs(599)).await;
        assert!(!task.is_finished(), "自管工具在兜底上限前不应被判定失败");
        tokio::time::advance(Duration::from_secs(2) + SETTLE_CEILING).await;
        let error = task.await.expect("join").expect_err("必须返回错误");
        assert_eq!(error.retryable(), Some(false));
        assert!(
            error.message().contains("自管预算上限"),
            "错误应说明自管预算上限：{}",
            error.message()
        );
        tokio::task::yield_now().await;
        assert_eq!(
            dropped.load(Ordering::SeqCst),
            0,
            "上限到达不得丢弃仍在收敛的在途执行"
        );
    }

    /// 调度器路径：enqueue 准入产出的 effective 参数随 `ToolInvocationContext`
    /// 流入，execute 直接消费——`prepare_arguments` 不重算，且工具闭包收到的
    /// 正是这份 effective 值。
    #[tokio::test]
    async fn execute_reuses_prepared_arguments_without_recompute() {
        use super::super::invocation::ToolInvocationContext;
        use crate::agent::rig_ext::tools::run_record::PREPARE_ARGUMENTS_CALLS;

        let (policy, _dir) = policy();
        let received = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));
        let captured = received.clone();
        let tool = PortableDynamicTool::new(
            "read_file",
            "捕获参数",
            json!({"type":"object"}),
            move |arguments| {
                let captured = captured.clone();
                Box::pin(async move {
                    *captured.lock().unwrap() = Some(arguments);
                    Ok(ToolOutput::text("ok"))
                })
            },
        );
        let call = ToolCall::from_wire(
            "call",
            ToolFunction {
                name: "read_file".into(),
                arguments: json!({ "paths": ["a.txt"] }),
            },
        );
        let prepared = json!({ "paths": ["a.txt"], "offset": 1 });
        let (_cancel, cancel_rx) = watch::channel(false);
        let context = ToolInvocationContext {
            workspace_id: "workspace".into(),
            agent_run_id: "run".into(),
            task_id: "task".into(),
            tool_call_id: "call".into(),
            root_request_message_id: "request".into(),
            cancel_rx,
            prepared_arguments: Some(prepared.clone()),
        };
        PREPARE_ARGUMENTS_CALLS.with(|count| count.set(0));
        context
            .scope(policy.execute(&tool, &call))
            .await
            .expect("执行成功");
        assert_eq!(
            PREPARE_ARGUMENTS_CALLS.with(std::cell::Cell::get),
            0,
            "execute 应复用 context 携带的 prepared 参数，不得重算"
        );
        assert_eq!(
            received.lock().unwrap().clone(),
            Some(prepared),
            "工具闭包收到的必须是 enqueue 产出的 effective 参数"
        );
    }

    /// 调度器路径：before_call 复用 enqueue 准入结果——校验门不再重查
    /// （准入已用同一纯函数对同一不可变输入校验）。
    #[tokio::test]
    async fn before_call_reuses_prepared_arguments_without_recompute() {
        use super::super::invocation::ToolInvocationContext;
        use crate::agent::rig_ext::tools::run_record::PREPARE_ARGUMENTS_CALLS;

        let (policy, _dir) = policy();
        let tool = PortableDynamicTool::new(
            "read_file",
            "只读工具",
            json!({"type":"object"}),
            move |_| Box::pin(async move { Ok(ToolOutput::text("ok")) }),
        );
        let call = ToolCall::from_wire(
            "call",
            ToolFunction {
                name: "read_file".into(),
                arguments: json!({ "paths": ["a.txt"] }),
            },
        );
        let (_cancel, cancel_rx) = watch::channel(false);
        let context = ToolInvocationContext {
            workspace_id: "workspace".into(),
            agent_run_id: "run".into(),
            task_id: "task".into(),
            tool_call_id: "call".into(),
            root_request_message_id: "request".into(),
            cancel_rx,
            prepared_arguments: Some(json!({ "paths": ["a.txt"], "offset": 1 })),
        };
        PREPARE_ARGUMENTS_CALLS.with(|count| count.set(0));
        let guard = context.scope(policy.before_call(&tool, &call)).await;
        assert!(guard.rejection.is_none(), "已准入参数不应被门禁拒绝");
        assert_eq!(
            PREPARE_ARGUMENTS_CALLS.with(std::cell::Cell::get),
            0,
            "before_call 应复用 context 携带的 prepared 参数，不得重算"
        );
    }

    /// 无上下文调用：before_call 的校验门只做一次参数准备（原实现各算一次）。
    #[tokio::test]
    async fn bare_before_call_prepares_arguments_exactly_once() {
        use crate::agent::rig_ext::tools::run_record::PREPARE_ARGUMENTS_CALLS;

        let (policy, _dir) = policy();
        let tool = PortableDynamicTool::new(
            "read_file",
            "只读工具",
            json!({"type":"object"}),
            move |_| Box::pin(async move { Ok(ToolOutput::text("ok")) }),
        );
        let call = ToolCall::from_wire(
            "call",
            ToolFunction {
                name: "read_file".into(),
                arguments: json!({ "paths": ["a.txt"] }),
            },
        );
        PREPARE_ARGUMENTS_CALLS.with(|count| count.set(0));
        let guard = policy.before_call(&tool, &call).await;
        assert!(guard.rejection.is_none());
        assert_eq!(
            PREPARE_ARGUMENTS_CALLS.with(std::cell::Cell::get),
            1,
            "裸路径 before_call 的台账与校验门应共用一次参数准备"
        );
    }
}

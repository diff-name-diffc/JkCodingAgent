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

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

use rig::message::ToolCall;
use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use tauri::ipc::Channel;
use tokio::sync::watch;

use super::surface::{ToolCallGuard, ToolCallOutcome, ToolCallTrace, ToolExecutionPolicy};
use crate::agent::common::cancellation_requested;
use crate::agent::db::{DispatcherDb, ToolRunTraceContext};
use crate::agent::rig_ext::events::AgentEvent;
use crate::agent::rig_ext::review::RigReviewContext;
use crate::agent::rig_ext::tools::run_record::{
    finish_tool_run, prepare_arguments, start_tool_run, RigToolRun, RigToolRunContext,
    RigToolRunFinish,
};
use crate::agent::rig_ext::tools::spec::{ToolSafety, ToolSpec};

/// MCP 动态工具的 canonical 名前缀（见 `mcp/registry.rs`）。
const MCP_TOOL_NAME_PREFIX: &str = "mcp__";

/// 发出取消/超时信号后，在途工具执行收敛的兜底上限。
///
/// 统一超时由 `tools/spec.rs` 策略表声明（最长 60 秒），取消经 `watch` 通道即时
/// 下发，正常路径远早于此即收敛——这个上限只为兜住真正卡死的工具回调。取
/// 「最大统一超时的 10 倍」是为了不把正常的慢工具误判为未收敛；自管超时的工具
/// （`unified_timeout=false`）走下面的提前返回，不受此上限约束。
const SETTLE_CEILING: Duration = Duration::from_secs(600);

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
}

/// 应用级执行策略：借用 DB 与事件通道，持有门禁输入。
#[derive(Clone)]
pub struct AppToolExecutionPolicy {
    db: DispatcherDb,
    on_event: Channel<AgentEvent>,
    config: AppToolPolicyConfig,
}

impl AppToolExecutionPolicy {
    pub fn new(
        db: &DispatcherDb,
        on_event: &Channel<AgentEvent>,
        config: AppToolPolicyConfig,
    ) -> Self {
        Self {
            db: db.clone(),
            on_event: on_event.clone(),
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
                ToolSpec::mcp(
                    name.to_string(),
                    definition.description,
                    definition.parameters,
                ),
                true,
            );
        }
        let definition = tool.definition();
        let registered = crate::agent::rig_ext::tools::spec::is_registered_tool_name(name);
        (
            ToolSpec::new(name, &definition.description, definition.parameters),
            registered,
        )
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
        let (spec, registered) = self.spec_for(tool);
        let tool_call_id = call.wire_call_id().to_string();
        let run_context = RigToolRunContext {
            db: &self.db,
            workspace_id: &self.config.workspace_id,
            on_event: &self.on_event,
        };

        // 1. 台账创建 + started：无效参数同样先进入台账（旧实现口径）。
        let effective_arguments =
            prepare_arguments(&spec.name, &spec.parameters, &call.function.arguments)
                .unwrap_or_else(|_| call.function.arguments.clone());
        let registered_context = super::invocation::ToolInvocationContext::current();
        let trace = if let Some(context) = registered_context {
            Some(ToolCallTrace {
                run_id: Some(context.task_id),
            })
        } else {
            match start_tool_run(
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
                    return ToolCallGuard {
                        trace: None,
                        rejection: Some(
                            ToolExecutionError::other(format!(
                                "错误：创建工具运行记录失败（工具 {}），未执行工具：{error}",
                                spec.name
                            ))
                            .with_code("fatal"),
                        ),
                    };
                }
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
        let arguments = prepare_arguments(&spec.name, &spec.parameters, &call.function.arguments)
            .map_err(|error| ToolExecutionError::invalid_args(error.message))?;

        // 等待上限：统一超时工具用自身 `timeout_secs`；自管工具（unified_timeout=false）用
        // 兜底上限 `settle_ceiling_secs`（工具自身预算之外的最后防线，取值 = 最坏合法预算）。
        // 两者共用同一套「到点 → 发取消 → 宽限收敛 → 交接后台」流程，差别只在文案与
        // 「是否值得发取消」：统一超时工具恒发，自管工具按 `cancellable` 声明。
        // `timeout_secs == 0` 维持文档语义（不设统一超时限制）；策略表内工具恒 > 0。
        let ceiling_mode = !spec.execution.unified_timeout;
        let deadline_secs = if ceiling_mode {
            spec.execution.settle_ceiling_secs
        } else {
            spec.execution.timeout_secs
        };
        if deadline_secs == 0 {
            return tool.execute(arguments).await;
        }
        let deadline = Duration::from_secs(deadline_secs);
        let invocation = super::invocation::ToolInvocationContext::current();
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
            _ = cancellation(upstream) => {
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
                db: &self.db,
                workspace_id: &self.config.workspace_id,
                on_event: &self.on_event,
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

async fn cancellation(rx: Option<watch::Receiver<bool>>) {
    let Some(mut rx) = rx else {
        return std::future::pending().await;
    };
    while !*rx.borrow() {
        if rx.changed().await.is_err() {
            break;
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

    fn policy() -> AppToolExecutionPolicy {
        let temp_dir =
            std::env::temp_dir().join(format!("rig-app-policy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).expect("create temp dir");
        let db = DispatcherDb::new(temp_dir.join("jkbot.sqlite3")).expect("open temp db");
        let events = Channel::new(|_| Ok(()));
        AppToolExecutionPolicy::new(
            &db,
            &events,
            AppToolPolicyConfig {
                workspace_id: "workspace".into(),
                workspace: temp_dir,
                review: RigReviewContext::unconfigured(),
                cancel_rx: None,
                trace: Default::default(),
            },
        )
    }

    /// 上限只兜住真正卡死的工具：发出取消信号后仍不收敛时，不丢弃在途执行
    /// （移交后台继续收敛），调用方拿到明确的「结算未确认」错误而不是无限等待。
    #[tokio::test(start_paused = true)]
    async fn settle_ceiling_hands_off_stuck_execution_without_dropping_it() {
        let policy = policy();
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
        let policy = policy();
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
}

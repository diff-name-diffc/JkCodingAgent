//! 批次配对、执行与结果持久化。
use super::support::{tool_error_text, tool_output_text};
use super::{
    invocation, RigLoopHooks, RigProtocolAction, RigToolSurface, ToolCallOutcome,
    ToolExecutionPolicy,
};
use crate::agent::common::{cancellation_requested, emit, UsageTracker};
use crate::agent::db::{DispatcherDb, OutboundToolCall};
use crate::agent::rig_ext::events::AgentEvent;
use crate::agent::rig_ext::tool_result::{persist_rig_tool_result, RigSummaryModel};
use anyhow::Result;
use rig::completion::CompletionModel;
use rig::message::{ToolCall, ToolResult, ToolResultContent, UserContent};
use rig::tool::ToolExecutionError;
use tauri::ipc::Channel;
use tokio::sync::watch;

/// 一批工具调用的执行结果。
pub(super) struct ToolBatch {
    /// 回灌给模型的工具结果内容；None = 工具间取消（已执行结果已落库）。
    pub(super) contents: Option<Vec<UserContent>>,
    /// 本批最后一个落库工具结果的消息 id（滚动摘要持久化的覆盖锚点跟踪）。
    pub(super) last_result_message_id: Option<String>,
    /// 本批协议动作（编排器：图已提交）。
    pub(super) actions: Vec<RigProtocolAction>,
    /// 本批最终答复（`message` 工具）。
    pub(super) final_message: Option<String>,
    /// 本批是否出现可重试错误（含协议拒绝与普通工具的可重试失败）。
    pub(super) saw_retryable_error: bool,
}

/// 逐个执行工具调用并落库结果（取消时返回 contents=None：已执行结果已落库，
/// 剩余调用不再执行——循环随之收口，不会再发起带悬空 tool_calls 的请求）。
#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_tool_calls<S, P>(
    db: &DispatcherDb,
    workspace_id: &str,
    on_event: &Channel<AgentEvent>,
    tool_calls: &[ToolCall],
    outbound_calls: &[OutboundToolCall],
    surface: &RigToolSurface,
    tool_policy: &P,
    summary: Option<&RigSummaryModel<'_, S>>,
    usage_tracker: &mut UsageTracker,
    cancel_rx: &watch::Receiver<bool>,
    hooks: &RigLoopHooks,
    agent_run_id: &str,
    root_request_message_id: &str,
) -> Result<ToolBatch>
where
    S: CompletionModel,
    P: ToolExecutionPolicy,
{
    let mut result_contents = Vec::with_capacity(tool_calls.len());
    let mut last_result_message_id: Option<String> = None;
    let mut actions: Vec<RigProtocolAction> = Vec::new();
    let mut final_message: Option<String> = None;
    let mut saw_retryable_error = false;

    for (index, (call, outbound)) in tool_calls.iter().zip(outbound_calls).enumerate() {
        if cancellation_requested(cancel_rx) {
            // 本批未执行的剩余调用必须补齐占位结果：assistant 消息已连同全部
            // tool_calls 落库，缺结果会让下一次请求被服务端以 400 拒绝。
            persist_skipped_tool_results(
                db,
                workspace_id,
                on_event,
                &tool_calls[index..],
                &outbound_calls[index..],
                surface,
                summary,
                usage_tracker,
                "本轮运行已取消。",
            )
            .await?;
            return Ok(ToolBatch {
                contents: None,
                last_result_message_id,
                actions,
                final_message,
                saw_retryable_error,
            });
        }
        emit(
            on_event,
            AgentEvent::ToolStarted {
                task_id: None,
                tool_call_id: outbound.id.clone(),
                name: outbound.function.name.clone(),
                arguments: outbound.function.arguments.clone(),
            },
        );

        // 三段式策略：before_call（门禁 + 台账开始）→ execute → after_call（台账收尾）。
        let mut status: &'static str = "succeeded";
        let mut error_kind: Option<&'static str> = None;
        let mut fatal_message: Option<String> = None;
        let mut trace = None;

        // 协议工具拦截（编排器）：命中则不执行壳工具回调，由宿主完成真实动作。
        let protocol_result = match hooks.protocol_handler.as_ref() {
            Some(handler) => {
                handler
                    .handle(&call.function.name, &call.function.arguments)
                    .await
            }
            None => None,
        };

        let result_text = match protocol_result {
            Some(protocol) => {
                if protocol.retryable_error {
                    saw_retryable_error = true;
                    status = "recoverable_error";
                    error_kind = Some("recoverable_error");
                }
                actions.extend(protocol.actions);
                if protocol.final_message.is_some() {
                    final_message = protocol.final_message;
                }
                protocol.text
            }
            None => match surface.find(&call.function.name) {
                None => {
                    status = "recoverable_error";
                    error_kind = Some("recoverable_error");
                    format!("错误：未注册的工具：{}", call.function.name)
                }
                Some(tool) => {
                    let guard = tool_policy.before_call(tool, call).await;
                    trace = guard.trace;
                    match guard.rejection {
                        Some(error) => {
                            let (mapped_status, mapped_kind, fatal) = classify_tool_error(&error);
                            status = mapped_status;
                            error_kind = Some(mapped_kind);
                            if fatal {
                                fatal_message = Some(tool_error_text(&error));
                            }
                            tool_error_text(&error)
                        }
                        None => match (invocation::ToolInvocationContext {
                            workspace_id: workspace_id.to_string(),
                            agent_run_id: agent_run_id.to_string(),
                            task_id: trace
                                .as_ref()
                                .and_then(|trace| trace.run_id.clone())
                                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                            tool_call_id: call.wire_call_id().to_string(),
                            root_request_message_id: root_request_message_id.to_string(),
                            cancel_rx: cancel_rx.clone(),
                        })
                        .scope(tool_policy.execute(tool, call))
                        .await
                        {
                            Ok(output) => tool_output_text(&output),
                            Err(error) => {
                                let (mapped_status, mapped_kind, fatal) =
                                    classify_tool_error(&error);
                                status = mapped_status;
                                error_kind = Some(mapped_kind);
                                if error.retryable() == Some(true) {
                                    saw_retryable_error = true;
                                }
                                if fatal {
                                    fatal_message = Some(tool_error_text(&error));
                                }
                                tool_error_text(&error)
                            }
                        },
                    }
                }
            },
        };

        let policy = surface.policy_for(&call.function.name);
        let record = persist_rig_tool_result(
            db,
            workspace_id,
            on_event,
            call,
            &policy,
            &result_text,
            summary,
            usage_tracker,
        )
        .await?;
        last_result_message_id = Some(record.id.clone());

        // 台账收尾：结果已落库后回填 result_mode / message_id（对齐旧
        // `persist_and_finalize_executed_tool` 的调用顺序）。
        tool_policy
            .after_call(
                trace.as_ref(),
                call,
                ToolCallOutcome {
                    status,
                    result_mode: record.tool_result_mode.as_deref(),
                    message_id: Some(record.id.as_str()),
                    error_kind,
                    error_message: error_kind.map(|_| result_text.as_str()),
                    action_kind: None,
                },
            )
            .await;

        result_contents.push(UserContent::ToolResult(ToolResult {
            call: call.id.clone(),
            provider: call.provider.clone(),
            name: call.function.name.clone(),
            // 回灌模型的内容取 context_payload（压缩/截断后的形态），与
            // `load_llm_history` 的历史口径一致；完整原文在工具产物中。
            content: vec![ToolResultContent::text(
                record
                    .context_payload
                    .clone()
                    .unwrap_or_else(|| record.plain_text()),
            )],
        }));

        // 致命工具失败：本批已执行结果全部落库与收尾后中止 run
        //（对齐旧 `ExecutedToolFinalize::FatalTool` 的收口时机）。剩余调用同样
        // 补占位结果，否则历史里会留下未应答的 tool_calls。
        if let Some(message) = fatal_message {
            persist_skipped_tool_results(
                db,
                workspace_id,
                on_event,
                &tool_calls[index + 1..],
                &outbound_calls[index + 1..],
                surface,
                summary,
                usage_tracker,
                "本批次因前序工具致命失败已中止。",
            )
            .await?;
            anyhow::bail!("{message}");
        }
    }

    Ok(ToolBatch {
        contents: Some(result_contents),
        last_result_message_id,
        actions,
        final_message,
        saw_retryable_error,
    })
}

/// 为本批「未执行」的调用补齐占位工具结果。
///
/// assistant 消息在流式结束时已连同本批全部 tool_calls 落库；后面的调用若没有
/// 结果行，下一轮请求会因「assistant tool_calls 之后必须跟齐 tool 消息」被服务端
/// 以 400 拒绝（库中既有的残缺历史由 `repair_tool_call_pairing` 在读侧兜底，
/// 新产生的残缺在此写侧补齐）。措辞与运行取消时的门禁拒绝保持同一族。
#[allow(clippy::too_many_arguments)]
async fn persist_skipped_tool_results<M: CompletionModel>(
    db: &DispatcherDb,
    workspace_id: &str,
    on_event: &Channel<AgentEvent>,
    skipped_calls: &[ToolCall],
    skipped_outbound: &[OutboundToolCall],
    surface: &RigToolSurface,
    summary: Option<&RigSummaryModel<'_, M>>,
    usage_tracker: &mut UsageTracker,
    reason: &str,
) -> Result<()> {
    for (call, outbound) in skipped_calls.iter().zip(skipped_outbound) {
        emit(
            on_event,
            AgentEvent::ToolStarted {
                task_id: None,
                tool_call_id: outbound.id.clone(),
                name: outbound.function.name.clone(),
                arguments: outbound.function.arguments.clone(),
            },
        );
        let text = format!("错误：工具 '{}' 尚未执行。{reason}", call.function.name);
        let policy = surface.policy_for(&call.function.name);
        persist_rig_tool_result(
            db,
            workspace_id,
            on_event,
            call,
            &policy,
            &text,
            summary,
            usage_tracker,
        )
        .await?;
    }
    Ok(())
}

/// rig 工具错误 → 台账终态（status, error_kind, 是否致命）。
///
/// 词表对齐旧工具状态词表：succeeded / recoverable_error /
/// fatal_error / cancelled。致命语义经 `with_code("fatal")` 显式声明
/// （如子智能体委派失败）——rig 无 fatal 概念，用错误码承载。
pub(super) fn classify_tool_error(
    error: &ToolExecutionError,
) -> (&'static str, &'static str, bool) {
    if error.kind() == rig::tool::ToolErrorKind::Cancelled {
        return ("cancelled", "cancelled", false);
    }
    if error.code() == Some("fatal") {
        return ("fatal_error", "fatal_error", true);
    }
    ("recoverable_error", "recoverable_error", false)
}

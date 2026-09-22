//! 运行时循环（T1.3）：基于 rig 契约组合的多轮工具循环。
//!
//! 分层对齐旧运行循环的语义骨架，但不复用其
//! 类型：每轮迭代 = 组装请求（附加本轮工具图片 → vision 切换探测）→
//! `model.stream()` 消费 `StreamedAssistantContent`（Text/ReasoningDelta 增量
//! 发 `AssistantDelta`/`AssistantThinkingDelta`，seq 共享同一 message_id 计数器）
//! → 聚合 choice 拆分正文/思考/工具调用 → 有工具调用则落库 assistant 消息、
//! 逐个执行并落库结果、以 `UserContent::ToolResult` 回灌；无工具调用则落库
//! assistant 消息收口。取消对齐 `handle_cancelled_loop` 语义（落库部分消息 +
//! Finished）；超限/错误以 Failed 收口。
//!
//! 终止事件契约：本循环自带收口——成功/取消发 `Finished`，失败发 `Failed`
//! （旧架构中由 `run_agent_turn` 发出；新循环自包含以便 Phase 3 各 agent 直接复用）。
//!
//! 子模块：`surface`（工具面与执行策略）、`stream`（rig 流消费与聚合拆分）、
//! `support`（消息组装/落库辅助）。

use anyhow::Result;
use rig::completion::{CompletionModel, Message};
use rig::message::{ToolCall, ToolResult, ToolResultContent, UserContent};
use rig::tool::ToolExecutionError;
use tauri::ipc::Channel;
use tokio::sync::watch;

use super::message::attach_turn_tool_images;
use super::model::{build_completion_request, ModelSelectionHandle, PurposeModelSpec};
use super::tool_result::{persist_rig_tool_result, RigSummaryModel};
use crate::agent::common::{
    cancellation_requested, emit, persist_assistant_message, persist_tool_calls_message,
    UsageTracker,
};
use crate::agent::db::OutboundToolCall;
use crate::agent::db::{DispatcherDb, DispatcherMessageRecord, DispatcherSessionTokenUsageSource};
use crate::agent::rig_ext::events::AgentEvent;
use crate::agent::rig_ext::tools::MAX_TOOL_CALLS_PER_BATCH;
use crate::shared::error::format_anyhow_error;

mod app_policy;
mod protocol;
mod stream;
mod support;
mod surface;
#[cfg(test)]
mod tests;

pub use app_policy::{AppToolExecutionPolicy, AppToolPolicyConfig};
pub use protocol::{ProtocolToolHandler, RigProtocolAction, RigProtocolResult};
pub use surface::{RigToolSurface, ToolCallOutcome, ToolExecutionPolicy};

use stream::{consume_stream, split_choice};
use support::{
    build_assistant_message, current_model_name, emit_finished, finalize_cancelled,
    format_finish_reason, maybe_emit_model_switched, outbound_tool_call, record_rig_usage,
    tool_error_text, tool_output_text,
};

// ─── 循环钩子 ─────────────────────────────────────────────────────────────────

/// 空响应（无正文且无工具调用）的诊断上下文，供 `empty_response_error` 构造文案。
pub struct RigTurnDiagnostics {
    pub model_name: String,
    pub finish_reason: Option<String>,
    pub thinking_chars: usize,
    pub completion_tokens: Option<u64>,
}

/// 循环的可注入钩子（Phase 3 各 agent 的差异点集中于此）。
pub struct RigLoopHooks {
    /// 最大工具迭代次数（对齐 `DispatcherAgentConfig::max_tool_iterations`）。
    pub max_iterations: usize,
    /// 单轮工具调用数上限（对齐 `MAX_TOOL_CALLS_PER_BATCH`）。
    pub max_tool_calls_per_batch: usize,
    /// 请求采样/容量参数：模型非 `PurposeSwitchingModel` 时生效；
    /// 是 PurposeSwitchingModel 时以命中槽位为准（见 model.rs 的 tune_request）。
    pub request_max_tokens: Option<u64>,
    pub request_temperature: f64,
    pub request_enable_thinking: bool,
    /// 每轮迭代的系统提示（preamble）。普通聊天每轮重建系统提示（G9-17：
    /// 系统时间/分类上下文等动态内容不随 run 陈旧），故为闭包而非静态值。
    pub preamble_for_iteration: Option<Box<dyn FnMut(usize) -> Option<String> + Send + Sync>>,
    /// 取消收口文案：输入已流出的部分正文，输出落库的 assistant 文本。
    pub cancelled_reply: Box<dyn Fn(&str) -> String + Send + Sync>,
    /// 空响应错误构造。
    pub empty_response_error: Box<dyn Fn(&RigTurnDiagnostics) -> String + Send + Sync>,
    /// 达到迭代上限的错误文案（None 用默认）。
    pub max_iterations_error: Option<String>,
    /// 用量落库来源（primary / summary）。
    pub usage_source: DispatcherSessionTokenUsageSource,
    /// `PurposeSwitchingModel` 的选择探测句柄：ModelSwitched 事件与真实用量模型名。
    pub model_selection: Option<ModelSelectionHandle>,
    /// 无探测句柄时的模型名（用量落库 / 诊断）。
    pub default_model_name: String,
    /// 上下文窗口容量（tokens），随用量落库。
    pub context_window: Option<u64>,
    /// 协议工具处理器（编排器注入：submit_graph / graph_plan_report / message）。
    /// None（聊天路径）= 全部工具按普通工具执行。
    pub protocol_handler: Option<std::sync::Arc<dyn ProtocolToolHandler>>,
}

impl RigLoopHooks {
    /// 以聊天槽位规格构造默认钩子（文案对齐 plain_chat 语义）。
    pub fn from_chat_spec(spec: &PurposeModelSpec) -> Self {
        Self {
            max_iterations: 200,
            max_tool_calls_per_batch: MAX_TOOL_CALLS_PER_BATCH,
            request_max_tokens: spec.max_tokens,
            request_temperature: spec.temperature,
            request_enable_thinking: spec.enable_thinking,
            preamble_for_iteration: None,
            cancelled_reply: Box::new(default_cancelled_reply),
            empty_response_error: Box::new(default_empty_response_error),
            max_iterations_error: None,
            usage_source: DispatcherSessionTokenUsageSource::Primary,
            model_selection: None,
            default_model_name: spec.model.clone(),
            context_window: spec.context_window,
            protocol_handler: None,
        }
    }
}

/// 对齐旧 `build_stopped_plain_chat_reply` 的取消收口文案。
fn default_cancelled_reply(partial: &str) -> String {
    let trimmed = partial.trim();
    if trimmed.is_empty() {
        "⏹️ 本轮聊天已停止。当前会话上下文已保留，可稍后继续。".to_string()
    } else {
        format!(
            "{}\n\n[本轮聊天已手动停止。当前会话上下文与以上输出均已保留，可稍后继续。]",
            trimmed
        )
    }
}

fn default_empty_response_error(diagnostics: &RigTurnDiagnostics) -> String {
    format!(
        "LLM 返回了空响应且没有工具调用，无法继续执行。\n请求摘要：model={}\n诊断：finish_reason={}，思考链={} 字符，completion_tokens={}",
        diagnostics.model_name,
        diagnostics.finish_reason.as_deref().unwrap_or("<未提供>"),
        diagnostics.thinking_chars,
        diagnostics
            .completion_tokens
            .map(|tokens| tokens.to_string())
            .unwrap_or_else(|| "<未上报>".to_string()),
    )
}

// ─── 主循环 ───────────────────────────────────────────────────────────────────

/// 运行一轮多轮工具循环，返回收口的 assistant 落库消息。
///
/// 终止事件：成功/取消发 `Finished`，失败发 `Failed`（自包含收口）。
#[allow(clippy::too_many_arguments)]
pub async fn run_rig_loop<M, S, P>(
    db: &DispatcherDb,
    workspace_id: &str,
    model: &M,
    messages: Vec<Message>,
    surface: &RigToolSurface,
    tool_policy: &P,
    summary: Option<&RigSummaryModel<'_, S>>,
    hooks: &mut RigLoopHooks,
    on_event: &Channel<AgentEvent>,
    cancel_rx: watch::Receiver<bool>,
    usage_tracker: &mut UsageTracker,
) -> Result<DispatcherMessageRecord>
where
    M: CompletionModel,
    S: CompletionModel,
    P: ToolExecutionPolicy,
{
    let result = run_loop_inner(
        db,
        workspace_id,
        model,
        messages,
        surface,
        tool_policy,
        summary,
        hooks,
        on_event,
        cancel_rx,
        usage_tracker,
    )
    .await;

    if let Err(error) = &result {
        emit(
            on_event,
            AgentEvent::Failed {
                workspace_id: workspace_id.to_string(),
                // format_anyhow_error 保留完整错误链（HTTP 状态、响应体、连接失败等根因），
                // 对齐旧 run_agent_turn 的 Failed 收口。
                message: format_anyhow_error(error),
            },
        );
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn run_loop_inner<M, S, P>(
    db: &DispatcherDb,
    workspace_id: &str,
    model: &M,
    mut messages: Vec<Message>,
    surface: &RigToolSurface,
    tool_policy: &P,
    summary: Option<&RigSummaryModel<'_, S>>,
    hooks: &mut RigLoopHooks,
    on_event: &Channel<AgentEvent>,
    cancel_rx: watch::Receiver<bool>,
    usage_tracker: &mut UsageTracker,
) -> Result<DispatcherMessageRecord>
where
    M: CompletionModel,
    S: CompletionModel,
    P: ToolExecutionPolicy,
{
    for iteration in 0..hooks.max_iterations {
        if cancellation_requested(&cancel_rx) {
            // 循环边界取消时尚未开始流式输出，无 delta 序号可对账。
            return finalize_cancelled(db, workspace_id, on_event, hooks, usage_tracker, "", None)
                .await;
        }

        // 本轮工具图片附加（chat-image:// 引用 → 视觉输入），vision 槽位切换随之命中。
        let effective_messages = attach_turn_tool_images(&messages).await;
        let preamble = hooks
            .preamble_for_iteration
            .as_mut()
            .and_then(|build| build(iteration));
        let request = build_completion_request(
            preamble,
            effective_messages,
            surface.definitions(),
            hooks.request_max_tokens,
            hooks.request_temperature,
            hooks.request_enable_thinking,
        );

        let mut stream = model.stream(request).await.map_err(|error| {
            anyhow::anyhow!(
                "LLM 流式请求失败（model={}）：{error}",
                current_model_name(hooks)
            )
        })?;

        // ModelSwitched：仅首轮通知（对齐 select_provider_for_messages 的
        // notify_user = iteration == 0；与聊天 provider 完全一致不通知）。
        if iteration == 0 {
            maybe_emit_model_switched(hooks, on_event);
        }

        let consumption = consume_stream(&mut stream, on_event, cancel_rx.clone()).await?;
        if consumption.cancelled {
            return finalize_cancelled(
                db,
                workspace_id,
                on_event,
                hooks,
                usage_tracker,
                &consumption.partial_text,
                consumption.last_seq,
            )
            .await;
        }

        // 用量：零值是 rig 文档化的「未上报」哨兵，不记录。
        if let Some(usage) = stream
            .response
            .as_ref()
            .map(|final_record| final_record.usage)
            .filter(|usage| usage.has_values())
        {
            record_rig_usage(
                db,
                workspace_id,
                &current_model_name(hooks),
                hooks.usage_source,
                &usage,
                hooks.context_window,
                usage_tracker,
                on_event,
            );
        }

        let (visible_text, thinking, tool_calls) = split_choice(&stream.choice);

        if tool_calls.is_empty() {
            if visible_text.is_empty() {
                let diagnostics = RigTurnDiagnostics {
                    model_name: current_model_name(hooks),
                    finish_reason: stream
                        .response
                        .as_ref()
                        .and_then(|final_record| final_record.finish_reason.as_ref())
                        .map(format_finish_reason),
                    thinking_chars: thinking.chars().count(),
                    completion_tokens: stream
                        .response
                        .as_ref()
                        .map(|final_record| final_record.usage.output_tokens),
                };
                anyhow::bail!("{}", (hooks.empty_response_error)(&diagnostics));
            }
            let usage_stats = usage_tracker.snapshot();
            let reply =
                persist_assistant_message(db, workspace_id, &visible_text, &usage_stats).await?;
            emit(
                on_event,
                AgentEvent::AssistantMessage {
                    message: reply.clone(),
                    last_seq: consumption.last_seq,
                },
            );
            emit_finished(db, workspace_id, on_event).await?;
            return Ok(reply);
        }

        if tool_calls.len() > hooks.max_tool_calls_per_batch {
            anyhow::bail!(
                "模型单轮返回 {} 个工具调用，超过运行时上限 {}；已在持久化或执行前拒绝。",
                tool_calls.len(),
                hooks.max_tool_calls_per_batch
            );
        }

        // G9-07/G9-14：tool_call_id 必填贯穿 Planned→Started→Finished；
        // 参数序列化失败上抛（不静默降级为 {}）。
        let outbound_calls = tool_calls
            .iter()
            .map(outbound_tool_call)
            .collect::<Result<Vec<_>>>()?;
        for call in &outbound_calls {
            emit(
                on_event,
                AgentEvent::ToolPlanned {
                    tool_call_id: call.id.clone(),
                    name: call.function.name.clone(),
                    arguments: call.function.arguments.clone(),
                },
            );
        }

        // 落库 assistant 工具调用消息（含思考），再向历史追加等价 rig 消息。
        persist_tool_calls_message(
            db,
            workspace_id,
            &visible_text,
            &outbound_calls,
            &thinking,
            Some(consumption.thinking_elapsed_ms),
        )
        .await?;
        messages.push(build_assistant_message(
            &visible_text,
            &thinking,
            &tool_calls,
            stream.message_id.clone(),
        ));

        let batch = execute_tool_calls(
            db,
            workspace_id,
            on_event,
            &tool_calls,
            &outbound_calls,
            surface,
            tool_policy,
            summary,
            usage_tracker,
            &cancel_rx,
            hooks,
        )
        .await?;
        let Some(result_contents) = batch.contents else {
            // 工具间取消：已执行结果已逐个落库；按取消语义收口（无部分正文——
            // 本轮流式已完整结束）。
            return finalize_cancelled(db, workspace_id, on_event, hooks, usage_tracker, "", None)
                .await;
        };
        messages.push(Message::User {
            content: result_contents,
        });

        // 协议收口（编排器）：动作 > 可重试错误 > 最终答复——三者优先级对齐旧
        // `resolve_loop_outcome`：已登记的图绝不因同轮另有可重试错误被丢弃；
        // 有可重试错误则让模型先自修复，不收口。
        if let Some(handler) = hooks.protocol_handler.as_ref() {
            let closing = if !batch.actions.is_empty() {
                handler
                    .render_outcome(&batch.actions, batch.final_message.as_deref())
                    .await
            } else if batch.saw_retryable_error {
                None
            } else if batch.final_message.is_some() {
                handler
                    .render_outcome(&[], batch.final_message.as_deref())
                    .await
            } else {
                None
            };
            if let Some(text) = closing {
                let usage_stats = usage_tracker.snapshot();
                let reply =
                    persist_assistant_message(db, workspace_id, &text, &usage_stats).await?;
                emit(
                    on_event,
                    AgentEvent::AssistantMessage {
                        message: reply.clone(),
                        // 工具循环后的合成收口消息，无关联的流式 delta 序号。
                        last_seq: None,
                    },
                );
                emit_finished(db, workspace_id, on_event).await?;
                return Ok(reply);
            }
        }
    }

    anyhow::bail!(
        "{}",
        hooks.max_iterations_error.clone().unwrap_or_else(|| {
            format!(
                "已达到最大工具迭代次数（{}），本轮执行被终止。请检查模型是否陷入工具调用循环。",
                hooks.max_iterations
            )
        })
    )
}

/// 一批工具调用的执行结果。
struct ToolBatch {
    /// 回灌给模型的工具结果内容；None = 工具间取消（已执行结果已落库）。
    contents: Option<Vec<UserContent>>,
    /// 本批协议动作（编排器：图已提交）。
    actions: Vec<RigProtocolAction>,
    /// 本批最终答复（`message` 工具）。
    final_message: Option<String>,
    /// 本批是否出现可重试错误（含协议拒绝与普通工具的可重试失败）。
    saw_retryable_error: bool,
}

/// 逐个执行工具调用并落库结果（取消时返回 contents=None：已执行结果已落库，
/// 剩余调用不再执行——循环随之收口，不会再发起带悬空 tool_calls 的请求）。
#[allow(clippy::too_many_arguments)]
async fn execute_tool_calls<S, P>(
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
) -> Result<ToolBatch>
where
    S: CompletionModel,
    P: ToolExecutionPolicy,
{
    let mut result_contents = Vec::with_capacity(tool_calls.len());
    let mut actions: Vec<RigProtocolAction> = Vec::new();
    let mut final_message: Option<String> = None;
    let mut saw_retryable_error = false;

    for (call, outbound) in tool_calls.iter().zip(outbound_calls) {
        if cancellation_requested(cancel_rx) {
            return Ok(ToolBatch {
                contents: None,
                actions,
                final_message,
                saw_retryable_error,
            });
        }
        emit(
            on_event,
            AgentEvent::ToolStarted {
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
                        None => match tool_policy.execute(tool, call).await {
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
        //（对齐旧 `ExecutedToolFinalize::FatalTool` 的收口时机）。
        if let Some(message) = fatal_message {
            anyhow::bail!("{message}");
        }
    }

    Ok(ToolBatch {
        contents: Some(result_contents),
        actions,
        final_message,
        saw_retryable_error,
    })
}

/// rig 工具错误 → 台账终态（status, error_kind, 是否致命）。
///
/// 词表对齐旧工具状态词表：succeeded / recoverable_error /
/// fatal_error / cancelled。致命语义经 `with_code("fatal")` 显式声明
/// （如子智能体委派失败）——rig 无 fatal 概念，用错误码承载。
fn classify_tool_error(error: &ToolExecutionError) -> (&'static str, &'static str, bool) {
    if error.kind() == rig::tool::ToolErrorKind::Cancelled {
        return ("cancelled", "cancelled", false);
    }
    if error.code() == Some("fatal") {
        return ("fatal_error", "fatal_error", true);
    }
    ("recoverable_error", "recoverable_error", false)
}

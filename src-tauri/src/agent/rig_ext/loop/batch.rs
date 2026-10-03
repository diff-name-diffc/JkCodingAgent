//! 批次配对、执行与结果持久化。
use super::{ProtocolToolHandler, RigProtocolAction, RigToolSurface};
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
    /// 本批是否出现可重试错误（含协议拒绝与宿主缺口短路）。
    pub(super) saw_retryable_error: bool,
}

/// 逐个执行协议批调用并落库结果（取消时返回 contents=None：已执行结果已
/// 落库，剩余调用不再执行——循环随之收口，不会再发起带悬空 tool_calls 的
/// 请求）。
///
/// 只承接**全协议批**（混批已在决策层拒绝），因此直接收 `ProtocolToolHandler`：
/// 命中的调用由宿主拦截完成真实动作；`handles` 命中但 `handle` 未拦截
/// （宿主缺口）时直接短路为可恢复错误——协议壳回调本就只报错（见
/// `agents/project_tools.rs`），不再回落执行壳工具，也不经策略层建台账：
/// 此处的 `before_call` 在 `ToolInvocationContext` 作用域之外，会走
/// `run_record` 裸路径产出 agent_run_id 为 NULL 的孤儿台账行（P0-3）。
#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_tool_calls<S>(
    db: &DispatcherDb,
    workspace_id: &str,
    on_event: &Channel<AgentEvent>,
    tool_calls: &[ToolCall],
    outbound_calls: &[OutboundToolCall],
    surface: &RigToolSurface,
    protocol_handler: &dyn ProtocolToolHandler,
    summary: Option<&RigSummaryModel<'_, S>>,
    usage_tracker: &mut UsageTracker,
    cancel_rx: &watch::Receiver<bool>,
) -> Result<ToolBatch>
where
    S: CompletionModel,
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

        // 协议工具拦截（编排器）：命中则不执行壳工具回调，由宿主完成真实动作。
        let result_text = match protocol_handler
            .handle(&call.function.name, &call.function.arguments)
            .await
        {
            Some(protocol) => {
                if protocol.retryable_error {
                    saw_retryable_error = true;
                }
                actions.extend(protocol.actions);
                if protocol.final_message.is_some() {
                    final_message = protocol.final_message;
                }
                protocol.text
            }
            None => {
                // 宿主缺口短路：免台账、不执行壳回调，回灌可恢复错误。
                saw_retryable_error = true;
                format!(
                    "错误：协议工具 '{}' 未被宿主拦截（handles 命中但 handle 未处理），本轮未执行。",
                    call.function.name
                )
            }
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

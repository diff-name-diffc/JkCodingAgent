//! 循环支撑函数：消息组装、落库契约转换、取消/完成收口、用量落库。

use anyhow::Result;
use rig::message::{AssistantContent, Message, Text, ToolCall, ToolResultContent};
use rig::tool::{ToolExecutionError, ToolOutput};
use tauri::ipc::Channel;

use super::super::llm_usage_from_rig;
use super::RigLoopHooks;
use crate::agent::common::{
    emit, persist_assistant_message, serialize_tool_arguments, UsageTracker,
};
use crate::agent::db::{DispatcherDb, DispatcherMessageRecord, DispatcherSessionTokenUsageSource};
use crate::agent::db::{FunctionCall, OutboundToolCall};
use crate::agent::rig_ext::events::AgentEvent;

/// 组装追加进历史的 assistant 消息：正文 + 工具调用。
/// 思考链不回灌上下文（瞬态产物：rig 的 openai 线格式会把 Reasoning 序列化进
/// 请求体，DeepSeek 等服务商明确要求历史不携带 reasoning_content；思考已随
/// `persist_tool_calls_message` 落库供 UI 展示，模型侧重放只会浪费预算）。
pub(super) fn build_assistant_message(
    visible_text: &str,
    tool_calls: &[ToolCall],
    message_id: Option<String>,
) -> Message {
    let mut content: Vec<AssistantContent> = Vec::new();
    if !visible_text.is_empty() {
        content.push(AssistantContent::Text(Text::new(visible_text)));
    }
    for call in tool_calls {
        content.push(AssistantContent::ToolCall(call.clone()));
    }
    Message::Assistant {
        id: message_id,
        content,
    }
}

/// rig ToolCall → 落库契约 OutboundToolCall（tool_calls_json 列的存储形态）。
pub(super) fn outbound_tool_call(call: &ToolCall) -> Result<OutboundToolCall> {
    Ok(OutboundToolCall {
        id: call.wire_call_id().to_string(),
        kind: "function".to_string(),
        function: FunctionCall {
            name: call.function.name.clone(),
            arguments: serialize_tool_arguments(&call.function.name, &call.function.arguments)?,
        },
    })
}

/// `ToolOutput` → 回灌文本：单文本块直取；多块拼接（JSON 块取其序列化文本，
/// 图片块占位——openai 线格式本就不支持工具结果携带图片）。
pub(super) fn tool_output_text(output: &ToolOutput) -> String {
    if let Some(text) = output.as_text() {
        return text.to_string();
    }
    output
        .as_content()
        .iter()
        .map(|content| match content {
            ToolResultContent::Text(text) => text.text.clone(),
            ToolResultContent::Json { value } => value.to_string(),
            ToolResultContent::Image(_) => "[图片结果]".to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 工具执行错误 → 模型可见文本。保持「错误：」前缀契约（前端
/// `is_tool_error_message` 据此识别可恢复错误）。
pub(super) fn tool_error_text(error: &ToolExecutionError) -> String {
    let text = tool_output_text(error.model_output());
    let trimmed = text.trim();
    if trimmed.is_empty() {
        "错误：工具执行失败".to_string()
    } else if trimmed.starts_with("错误：") {
        trimmed.to_string()
    } else {
        format!("错误：{trimmed}")
    }
}

/// 取消收口：落库停止回复 + AssistantMessage + Finished（对齐旧
/// `handle_cancelled_loop` → `emit_stop_and_finish` 语义）。
pub(super) async fn finalize_cancelled(
    db: &DispatcherDb,
    workspace_id: &str,
    on_event: &Channel<AgentEvent>,
    hooks: &RigLoopHooks,
    usage_tracker: &UsageTracker,
    partial: &str,
    last_seq: Option<u64>,
) -> Result<DispatcherMessageRecord> {
    let content = (hooks.cancelled_reply)(partial);
    let usage_stats = usage_tracker.snapshot();
    let reply = persist_assistant_message(db, workspace_id, &content, &usage_stats).await?;
    emit(
        on_event,
        AgentEvent::AssistantMessage {
            message: reply.clone(),
            last_seq,
        },
    );
    Ok(reply)
}

pub(super) async fn emit_finished(
    db: &DispatcherDb,
    workspace_id: &str,
    on_event: &Channel<AgentEvent>,
) -> Result<()> {
    let message_count = db.count_visible_messages_async(workspace_id).await?;
    emit(
        on_event,
        AgentEvent::Finished {
            workspace_id: workspace_id.to_string(),
            message_count,
        },
    );
    Ok(())
}

/// 首轮视觉切换通知（对齐 `select_provider_for_messages`：三项全同不通知）。
pub(super) fn maybe_emit_model_switched(hooks: &RigLoopHooks, on_event: &Channel<AgentEvent>) {
    let Some(selection) = hooks
        .model_selection
        .as_ref()
        .and_then(|handle| handle.last())
    else {
        return;
    };
    if selection.used_vision && selection.differs_from_chat {
        emit(
            on_event,
            AgentEvent::ModelSwitched {
                from_model: hooks.default_model_name.clone(),
                to_model: selection.model_name,
                reason: "检测到用户上传了图片".to_string(),
            },
        );
    }
}

/// 本次迭代实际使用的模型名（探测句柄优先，缺省回退聊天模型名）。
pub(super) fn current_model_name(hooks: &RigLoopHooks) -> String {
    hooks
        .model_selection
        .as_ref()
        .and_then(|handle| handle.last())
        .map(|selection| selection.model_name)
        .unwrap_or_else(|| hooks.default_model_name.clone())
}

/// 用量落库 + 追踪（对齐 `common::usage::record_usage`：DB 写入 fire-and-forget，
/// tracker 累积后立即发 RunUsageUpdated 快照）。
#[allow(clippy::too_many_arguments)]
pub(super) fn record_rig_usage(
    db: &DispatcherDb,
    workspace_id: &str,
    model: &str,
    source_kind: DispatcherSessionTokenUsageSource,
    usage: &rig::completion::Usage,
    context_window_capacity: Option<u64>,
    tracker: &mut UsageTracker,
    on_event: &Channel<AgentEvent>,
) {
    let llm_usage = llm_usage_from_rig(usage);
    let db = db.clone();
    let wid = workspace_id.to_string();
    let model = model.to_string();
    tokio::spawn(async move {
        if let Err(error) = db
            .upsert_session_token_usage_async(
                &wid,
                &model,
                source_kind,
                &llm_usage,
                context_window_capacity,
            )
            .await
        {
            eprintln!(
                "failed to persist session token usage for workspace {} and model {}: {}",
                wid, model, error
            );
        }
    });

    let stats = tracker.record(&llm_usage_from_rig(usage));
    emit(
        on_event,
        AgentEvent::RunUsageUpdated {
            workspace_id: workspace_id.to_string(),
            stats,
        },
    );
}

pub(super) fn format_finish_reason(reason: &rig::completion::FinishReason) -> String {
    match reason {
        rig::completion::FinishReason::Stop => "stop".to_string(),
        rig::completion::FinishReason::Length => "length".to_string(),
        rig::completion::FinishReason::ToolCalls => "tool_calls".to_string(),
        rig::completion::FinishReason::ContentFilter => "content_filter".to_string(),
        rig::completion::FinishReason::Other(other) => other.clone(),
    }
}

use serde_json::Value;
use tauri::ipc::Channel;

use crate::agent::common::{is_tool_error_message, UsageTracker};
use crate::agent::db::{DispatcherDb, DispatcherSessionTokenUsageSource, TOOL_RETRY_CONTEXT_PREFIX};
use crate::agent::llm::{LlmResponse, LlmUsage, RequestedToolCall};
use crate::agent::run_loop::AgentEvent;
use crate::shared::truncate_for_display;

pub(crate) fn extract_message_content(arguments: &Value) -> Option<String> {
    arguments
        .get("content")
        .and_then(Value::as_str)
        .map(str::to_string)
}

// ─── Tool Error Classification ────────────────────────────────────────────────

pub(crate) fn empty_llm_response_error(response: &LlmResponse) -> String {
    let raw_response = response.raw_response.trim();
    let response_detail = if raw_response.is_empty() {
        "<空>".to_string()
    } else {
        truncate_for_display(raw_response, 4_000, "\n...[LLM 接口响应内容已截断]")
    };

    format!("LLM 返回了空响应且没有工具调用，无法继续执行。\nLLM 接口响应内容：\n{response_detail}")
}

pub(crate) fn build_tool_retry_context(tool_call: &RequestedToolCall, error: &str) -> String {
    let arguments =
        serde_json::to_string_pretty(&tool_call.arguments).unwrap_or_else(|_| "{}".to_string());
    format!(
        "{TOOL_RETRY_CONTEXT_PREFIX}\n\
工具：{}\n\
工具调用 ID：{}\n\
错误详情：{}\n\n\
上次参数：\n{}\n\n\
要求：不要直接把该错误回复给用户。请根据工具 schema 和错误详情修正参数后重试同一个工具；重试成功后，系统会覆盖本次失败工具调用记录。",
        tool_call.name,
        tool_call.id,
        error.trim(),
        truncate_for_display(&arguments, 4_000, "\n...")
    )
}

pub(crate) fn is_retryable_tool_error(tool_name: &str, result: &str) -> bool {
    let trimmed = result.trim();
    if trimmed.is_empty() {
        return false;
    }
    if tool_name == "exec" {
        return false;
    }
    is_tool_error_message(trimmed)
}

// ─── Token Usage Recording ────────────────────────────────────────────────────

pub(crate) fn record_session_token_usage(
    db: &DispatcherDb,
    workspace_id: &str,
    model: &str,
    source_kind: DispatcherSessionTokenUsageSource,
    usage: &LlmUsage,
) {
    let db = db.clone();
    let wid = workspace_id.to_string();
    let m = model.to_string();
    let u = usage.clone();
    tokio::spawn(async move {
        if let Err(error) = db
            .upsert_session_token_usage_async(&wid, &m, source_kind, &u)
            .await
        {
            eprintln!(
                "failed to persist dispatcher session token usage for workspace {} and model {}: {}",
                wid, m, error
            );
        }
    });
}

pub(crate) fn record_run_token_usage(
    db: &DispatcherDb,
    workspace_id: &str,
    model: &str,
    source_kind: DispatcherSessionTokenUsageSource,
    usage: &LlmUsage,
    tracker: &mut UsageTracker,
    on_event: &Channel<AgentEvent>,
) {
    record_session_token_usage(db, workspace_id, model, source_kind, usage);
    let stats = tracker.record(usage);
    emit(
        on_event,
        AgentEvent::RunUsageUpdated {
            workspace_id: workspace_id.to_string(),
            stats,
        },
    );
}

pub(crate) fn normalize_summary_model(model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        crate::agent::config::DEFAULT_SUMMARY_MODEL.to_string()
    } else {
        trimmed.to_string()
    }
}

// ─── Event Helpers ────────────────────────────────────────────────────────────

pub(crate) fn emit(on_event: &Channel<AgentEvent>, event: AgentEvent) {
    let _ = on_event.send(event);
}

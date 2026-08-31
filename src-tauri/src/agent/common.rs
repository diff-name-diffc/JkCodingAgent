use tauri::ipc::Channel;
use tokio::sync::watch;

use super::llm::RequestedToolCall;
use super::run_loop::AgentEvent;
use super::tools::ToolRegistry;
use crate::mcp::McpScope;

mod message;
mod tool_result;
mod usage;

pub use message::{
    build_args_map, build_tool_calls_payload, persist_assistant_message,
    persist_tool_calls_message, select_provider_for_messages,
};
pub(crate) use message::{serialize_tool_arguments, should_keep_llm_message};
pub use tool_result::{persist_tool_result_with_compression, TOOL_RESULT_INLINE_MAX_CHARS};
#[cfg(test)]
use tool_result::{
    prepare_tool_result, TOOL_RESULT_INLINE_MAX_CHARS_PAGED, TOOL_RESULT_INLINE_MAX_CHARS_READ,
};
pub use usage::{stream_llm_response, with_usage_paused, LlmStreamOutcome, UsageTracker};

// ─── Cancellation ────────────────────────────────────────────────────────────────

pub fn cancellation_requested(cancel_rx: &watch::Receiver<bool>) -> bool {
    *cancel_rx.borrow() || cancel_rx.has_changed().is_err()
}

pub async fn wait_for_cancellation(cancel_rx: &mut watch::Receiver<bool>) {
    if cancellation_requested(cancel_rx) {
        return;
    }

    while cancel_rx.changed().await.is_ok() {
        if cancellation_requested(cancel_rx) {
            return;
        }
    }
}

// ─── Utility ─────────────────────────────────────────────────────────────────────

pub fn emit(on_event: &Channel<AgentEvent>, event: AgentEvent) {
    let _ = on_event.send(event);
}

pub fn is_parallel_readonly_tool_call(
    registry: &ToolRegistry,
    scope: &McpScope,
    tool_call: &RequestedToolCall,
) -> bool {
    registry.is_parallel_readonly(scope, &tool_call.name, true)
}

pub fn readonly_tool_run_end(
    registry: &ToolRegistry,
    scope: &McpScope,
    tool_calls: &[RequestedToolCall],
    start: usize,
) -> usize {
    tool_calls
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, tool_call)| {
            (!is_parallel_readonly_tool_call(registry, scope, tool_call)).then_some(index)
        })
        .unwrap_or(tool_calls.len())
}

// ─── Tool Outcome Classification ──────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
#[cfg(test)]
pub enum ToolOutcome {
    Ok,
    RecoverableError { message: String },
}

/// 文本层只能区分「错误：」前缀的可恢复错误；致命错误一律由类型化的
/// `ToolResult.status` 表达，不做文本推断。
#[cfg(test)]
pub fn classify_tool_result(result: &str) -> ToolOutcome {
    let trimmed = result.trim();
    if trimmed.is_empty() {
        return ToolOutcome::Ok;
    }
    if is_tool_error_message(trimmed) {
        return ToolOutcome::RecoverableError {
            message: trimmed.to_string(),
        };
    }
    ToolOutcome::Ok
}

pub(crate) fn is_tool_error_message(message: &str) -> bool {
    let message = message.trim();
    message.starts_with("错误：")
}

#[cfg(test)]
mod tests;

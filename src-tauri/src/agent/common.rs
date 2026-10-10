use tauri::ipc::Channel;
use tokio::sync::watch;

use crate::agent::rig_ext::events::AgentEvent;

mod message;
mod usage;

pub use message::{persist_assistant_message, persist_tool_calls_message};
pub(crate) use message::{
    repair_tool_call_pairing, serialize_tool_arguments, should_keep_llm_message,
    strip_delivered_wait_pairs, UNANSWERED_TOOL_RESULT_PLACEHOLDER, WAIT_TOOL_NAME,
};
pub use usage::UsageTracker;

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

/// `wait_for_cancellation` 的可选形：`None`（无取消源）时永不完成。
/// 策略层统一超时（loop/app_policy）与工具 HTTP 边界的 `select!` 分支
/// （media/image_api、media/fetch）共用本实现——取消等待语义单一出处，
/// 防「sender drop 处理」「置位检查」单侧漂移。
pub async fn wait_for_optional_cancellation(cancel_rx: Option<watch::Receiver<bool>>) {
    match cancel_rx {
        Some(mut rx) => wait_for_cancellation(&mut rx).await,
        None => std::future::pending().await,
    }
}

// ─── Utility ─────────────────────────────────────────────────────────────────────

pub fn emit(on_event: &Channel<AgentEvent>, event: AgentEvent) {
    let _ = on_event.send(event);
}

#[cfg(test)]
mod tests;

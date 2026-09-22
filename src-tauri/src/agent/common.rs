use tauri::ipc::Channel;
use tokio::sync::watch;

use crate::agent::rig_ext::events::AgentEvent;

mod message;
mod usage;

pub use message::{
    persist_assistant_message,
    persist_tool_calls_message,
};
pub(crate) use message::{serialize_tool_arguments, should_keep_llm_message};
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

// ─── Utility ─────────────────────────────────────────────────────────────────────

pub fn emit(on_event: &Channel<AgentEvent>, event: AgentEvent) {
    let _ = on_event.send(event);
}


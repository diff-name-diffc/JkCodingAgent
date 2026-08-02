use serde::Serialize;

use super::super::db::{
    DispatcherMessageRecord, DispatcherMessageUsageStats, DispatcherToolArtifactRef,
    DispatcherToolRunRecord,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurn {
    pub reply: DispatcherMessageRecord,
    pub messages: Vec<DispatcherMessageRecord>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "event",
    content = "data"
)]
pub enum AgentEvent {
    Started {
        workspace_id: String,
    },
    UserMessage {
        message: DispatcherMessageRecord,
    },
    AssistantStarted {
        message_id: String,
    },
    ModelSwitched {
        from_model: String,
        to_model: String,
        reason: String,
    },
    AssistantDelta {
        message_id: String,
        delta: String,
    },
    AssistantThinkingDelta {
        message_id: String,
        delta: String,
        elapsed_ms: u64,
    },
    AssistantMessage {
        message: DispatcherMessageRecord,
    },
    RunUsageUpdated {
        workspace_id: String,
        stats: DispatcherMessageUsageStats,
    },
    ToolPlanned {
        tool_call_id: Option<String>,
        name: String,
        arguments: String,
    },
    ToolStarted {
        tool_call_id: Option<String>,
        name: String,
        arguments: String,
    },
    #[allow(dead_code)]
    ToolSummaryStarted {
        tool_call_id: Option<String>,
        name: String,
        result_mode: String,
    },
    #[allow(dead_code)]
    ToolSummaryDelta {
        tool_call_id: Option<String>,
        name: String,
        delta: String,
        result_mode: String,
    },
    ToolFinished {
        tool_call_id: Option<String>,
        name: String,
        display_text: String,
        result_mode: String,
        detail_refs: Vec<DispatcherToolArtifactRef>,
    },
    ToolRunUpdated {
        run: DispatcherToolRunRecord,
    },
    Finished {
        messages: Vec<DispatcherMessageRecord>,
    },
    Failed {
        workspace_id: String,
        message: String,
    },
}

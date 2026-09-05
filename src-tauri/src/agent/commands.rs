use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde_json::Value;

use super::db::content::{segments_to_plain_text, try_parse_segments_json, ContentSegment};
use super::db::{
    AgentContext, AhaSettingsV2, ChatCategory, ChatCategoryAgentConfig, ChatSessionRecord,
    DispatcherDb, DispatcherMessageRecord, DispatcherModelConfig, DispatcherSessionKind,
    DispatcherSessionTokenUsageRecord, DispatcherSessionTokenUsageSource,
    DispatcherToolArtifactRecord, DispatcherToolRunRecord, KeywordAction, ProjectSessionRecord,
    SessionPage, SessionSearchResult,
};
use super::llm::OpenAiCompatProvider;
use super::llm::{self, ChatMessage};
use super::llm::{ChatMessageContentPart, ChatMessageImageSource};
use super::run_loop::{run_agent_turn, AgentEvent, AgentRunRequest, AgentTurn, RuntimeAgentKind};
use super::state::{DispatcherState, GenerationGuard};
use super::sub_agent::db::ToolInfo;
use super::summary::{
    fallback_session_title, parse_keyword_actions, summarize_session_keywords,
    summarize_session_title, SessionTitleMessage,
};
use crate::browser::BrowserManager;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter};

async fn run_dispatcher_db<T, F>(operation: &'static str, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|error| format!("{operation} task failed: {error}"))?
        .map_err(|error| error.to_string())
}

mod session_metadata;
#[cfg(test)]
use session_metadata::latest_qa_pair;
use session_metadata::{spawn_session_keywords_update, spawn_session_title_update};
pub(crate) mod architecture_commands;
pub(crate) mod category_commands;
pub(crate) mod message_commands;
pub(crate) mod model_commands;
pub(crate) mod run_commands;
pub(crate) mod session_commands;
pub(crate) mod settings_commands;

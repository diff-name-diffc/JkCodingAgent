//! 领域无关的共享常量、助手函数与行映射器。
//!
//! 这些工具被多个业务子模块共用，放在此处避免相互耦合。

use chrono::Utc;
use rusqlite::types::Type;

use crate::agent::llm::LlmUsage;

use super::artifacts::DispatcherToolArtifactRef;
use super::content::{parse_segments_json, segments_to_plain_text};
use super::messages::{DispatcherMessageRecord, DispatcherMessageUsageStats};

pub(super) const MAX_LLM_DIALOGUES: usize = 5;
pub(super) const MAX_DIALOGUE_QUERY_LIMIT: usize = 50;
pub(crate) const DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS: u64 = 1_000_000;
pub(crate) const TOOL_RETRY_CONTEXT_PREFIX: &str = "[工具调用失败，已交回模型修正重试]";

pub(super) fn now() -> String {
    Utc::now().to_rfc3339()
}

pub(super) fn default_context_window_capacity(_model: &str) -> u64 {
    DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS
}

pub(super) fn usage_total_tokens(usage: &LlmUsage) -> u64 {
    if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        usage.prompt_tokens + usage.completion_tokens
    }
}

// ── 行映射器 ──────────────────────────────────────────────────
// 这些映射器被 messages 等多个子模块共用，放在领域无关的此处。

pub(super) fn map_dispatcher_message_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<DispatcherMessageRecord> {
    let segments_json: String = row.get(3)?;
    let content = segments_to_plain_text(&parse_segments_json(&segments_json));
    let thinking_elapsed_ms = row
        .get::<_, Option<i64>>(5)?
        .map(u64::try_from)
        .transpose()
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(5, Type::Integer, Box::new(error))
        })?;

    Ok(DispatcherMessageRecord {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        role: row.get(2)?,
        content,
        segments_json,
        thinking_content: row.get(4)?,
        thinking_elapsed_ms,
        context_payload: row.get(6)?,
        tool_call_id: row.get(7)?,
        tool_name: row.get(8)?,
        tool_result_mode: row.get(9)?,
        tool_artifacts: parse_tool_artifact_refs(row.get::<_, Option<String>>(10)?),
        tool_calls_json: row.get(11)?,
        usage_stats: parse_message_usage_stats(row.get::<_, Option<String>>(12)?)?,
        created_at: row.get(13)?,
    })
}

fn parse_tool_artifact_refs(raw: Option<String>) -> Vec<DispatcherToolArtifactRef> {
    raw.as_deref()
        .and_then(|json| serde_json::from_str::<Vec<DispatcherToolArtifactRef>>(json).ok())
        .unwrap_or_default()
}

fn parse_message_usage_stats(
    raw: Option<String>,
) -> rusqlite::Result<Option<DispatcherMessageUsageStats>> {
    raw.as_deref()
        .filter(|json| !json.trim().is_empty())
        .map(|json| {
            serde_json::from_str::<DispatcherMessageUsageStats>(json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(10, Type::Text, Box::new(error))
            })
        })
        .transpose()
}

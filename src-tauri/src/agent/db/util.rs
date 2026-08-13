//! 领域无关的共享常量、助手函数与行映射器。
//!
//! 这些工具被多个业务子模块共用，放在此处避免相互耦合。

use chrono::Utc;
use rusqlite::types::Type;

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

// ── 行映射器 ──────────────────────────────────────────────────
// 这些映射器被 messages 等多个子模块共用，放在领域无关的此处。

pub(super) fn map_dispatcher_message_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<DispatcherMessageRecord> {
    // 按列名读取，SELECT 列序调整不会静默错位。
    let segments_json: String = row.get("segments_json")?;
    let content = segments_to_plain_text(&parse_segments_json(&segments_json));
    // 负值等脏数据容错为 0，避免单条坏行导致整个会话消息列表加载失败。
    let thinking_elapsed_ms = row
        .get::<_, Option<i64>>("thinking_elapsed_ms")?
        .map(|value| u64::try_from(value).unwrap_or(0));

    Ok(DispatcherMessageRecord {
        id: row.get("id")?,
        workspace_id: row.get("workspace_id")?,
        role: row.get("role")?,
        content,
        segments_json,
        thinking_content: row.get("thinking_content")?,
        thinking_elapsed_ms,
        context_payload: row.get("context_payload")?,
        tool_call_id: row.get("tool_call_id")?,
        tool_name: row.get("tool_name")?,
        tool_result_mode: row.get("tool_result_mode")?,
        tool_artifacts: parse_tool_artifact_refs(row.get::<_, Option<String>>(
            "tool_artifacts_json",
        )?),
        tool_calls_json: row.get("tool_calls_json")?,
        usage_stats: parse_message_usage_stats(
            row.get::<_, Option<String>>("usage_stats_json")?,
            row.as_ref().column_index("usage_stats_json").unwrap_or(0),
        )?,
        created_at: row.get("created_at")?,
    })
}

fn parse_tool_artifact_refs(raw: Option<String>) -> Vec<DispatcherToolArtifactRef> {
    let Some(json) = raw.as_deref().filter(|value| !value.trim().is_empty()) else {
        return Vec::new();
    };
    match serde_json::from_str::<Vec<DispatcherToolArtifactRef>>(json) {
        Ok(artifact_refs) => artifact_refs,
        Err(error) => {
            // 解析失败兜底为空列表，但必须留痕，否则产物引用丢失无从追查。
            eprintln!("parse_tool_artifact_refs failed: {error}");
            Vec::new()
        }
    }
}

fn parse_message_usage_stats(
    raw: Option<String>,
    column_index: usize,
) -> rusqlite::Result<Option<DispatcherMessageUsageStats>> {
    raw.as_deref()
        .filter(|json| !json.trim().is_empty())
        .map(|json| {
            serde_json::from_str::<DispatcherMessageUsageStats>(json).map_err(|error| {
                // 错误携带真实列索引，避免硬编码位置误导排查。
                rusqlite::Error::FromSqlConversionFailure(column_index, Type::Text, Box::new(error))
            })
        })
        .transpose()
}

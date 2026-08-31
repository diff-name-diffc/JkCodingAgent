//! 消息写入协议与公开 DTO。同步查询、清理事务和 `spawn_blocking` 异步适配
//! 分别位于子模块；私有核心插入逻辑（add_message / insert_tool_artifacts）留在
//! 本模块，避免暴露半初始化写入状态。

mod async_api;
mod cleanup;
mod queries;

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent::common::should_keep_llm_message;
use crate::agent::llm::{
    ChatMessage, ChatMessageContentPart, ChatMessageImageSource, OutboundToolCall,
};

use super::artifacts::{DispatcherToolArtifactRef, ToolArtifactDraft};
use super::content::{
    content_to_segments_json, delete_chat_image_resources, insert_chat_images, parse_segments_json,
    remove_chat_image_dir, segments_to_plain_text, try_parse_segments_json, ContentSegment,
};
use super::util::{map_dispatcher_message_record, now, MAX_LLM_DIALOGUES};
use super::DispatcherDb;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherMessageUsageStats {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub elapsed_ms: u64,
    #[serde(default)]
    pub paused: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherMessageRecord {
    pub id: String,
    pub workspace_id: String,
    pub role: String,
    pub segments_json: String,
    pub thinking_content: Option<String>,
    pub thinking_elapsed_ms: Option<u64>,
    pub context_payload: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_result_mode: Option<String>,
    pub tool_artifacts: Vec<DispatcherToolArtifactRef>,
    pub tool_calls_json: Option<String>,
    pub usage_stats: Option<DispatcherMessageUsageStats>,
    pub created_at: String,
}

impl DispatcherMessageRecord {
    /// 面向展示/消费的正文文本：从 segments_json 实时派生。segments 是消息
    /// 内容的唯一存储形态，不存在独立的 content 字段。
    pub fn plain_text(&self) -> String {
        segments_to_plain_text(&parse_segments_json(&self.segments_json))
    }

    pub fn to_llm_message(&self) -> Option<ChatMessage> {
        let (content, content_parts) = if let Some(payload) = self.context_payload.clone() {
            (payload, Vec::new())
        } else {
            let segments = parse_segments_json(&self.segments_json);
            (
                segments_to_plain_text(&segments),
                segments_to_llm_content_parts(&self.role, &segments),
            )
        };
        let tool_calls = self
            .tool_calls_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<Vec<OutboundToolCall>>(json).ok());
        let message = ChatMessage {
            reasoning_content: if self.role == "assistant" {
                self.thinking_content
                    .clone()
                    .filter(|content| !content.trim().is_empty())
            } else {
                None
            },
            role: self.role.clone(),
            content,
            content_parts,
            tool_call_id: self.tool_call_id.clone(),
            name: self.tool_name.clone(),
            tool_calls,
        };
        should_keep_llm_message(&message).then_some(message)
    }
}

fn segments_to_llm_content_parts(
    role: &str,
    segments: &[ContentSegment],
) -> Vec<ChatMessageContentPart> {
    if role != "user" {
        return Vec::new();
    }

    let mut parts = Vec::new();
    for segment in segments {
        match segment {
            ContentSegment::Text { text, .. } if !text.trim().is_empty() => {
                parts.push(ChatMessageContentPart::Text { text: text.clone() });
            }
            ContentSegment::Image { image_id, .. } => {
                parts.push(ChatMessageContentPart::Image {
                    source: ChatMessageImageSource::ChatImage {
                        image_id: image_id.clone(),
                    },
                });
                // 紧跟图片追加可回指的引用标注（见 chat_image_reference_note）。
                parts.push(ChatMessageContentPart::Text {
                    text: chat_image_reference_note(image_id),
                });
            }
            ContentSegment::Text { .. } | ContentSegment::File { .. } => {}
        }
    }
    parts
}

/// 图片段在 LLM 消息中的引用标注：多模态请求里图片以 data URL 发送，模型看不到
/// 任何可回指的标识；没有该标注时，Agent 想对会话图片调用
/// analyze_image / image_edit 只能凭空编造 image_id（曾把图片里的日志时间戳
/// 当成引用）。在图片部分之后追加文本引用，模型即可原样复制使用。
pub(crate) fn chat_image_reference_note(image_id: &str) -> String {
    format!(
        "[图片引用：chat-image://{image_id}，analyze_image / edit_image 等工具可直接使用该引用，请勿改写或编造]"
    )
}

struct NewDispatcherMessage<'a> {
    workspace_id: &'a str,
    role: &'a str,
    content: &'a str,
    segments_json: Option<String>,
    thinking_content: Option<&'a str>,
    thinking_elapsed_ms: u64,
    context_payload: Option<&'a str>,
    tool_call_id: Option<&'a str>,
    tool_name: Option<&'a str>,
    tool_result_mode: Option<&'a str>,
    tool_calls: Option<&'a [OutboundToolCall]>,
    tool_artifacts: &'a [ToolArtifactDraft],
    usage_stats: Option<&'a DispatcherMessageUsageStats>,
    visible: bool,
}

impl DispatcherDb {
    pub fn add_visible_message_from_segments(
        &self,
        workspace_id: &str,
        role: &str,
        segments_json: String,
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role,
            content: "",
            segments_json: Some(segments_json),
            thinking_content: None,
            thinking_elapsed_ms: 0,
            context_payload: None,
            tool_call_id: None,
            tool_name: None,
            tool_result_mode: None,
            tool_calls: None,
            tool_artifacts: &[],
            usage_stats: None,
            visible: true,
        })
    }

    pub fn add_visible_message_with_usage(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        usage_stats: &DispatcherMessageUsageStats,
    ) -> Result<DispatcherMessageRecord> {
        self.add_visible_message_with_usage_and_thinking(
            workspace_id,
            role,
            content,
            usage_stats,
            None,
            0,
        )
    }

    pub fn add_visible_message_with_usage_and_thinking(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        usage_stats: &DispatcherMessageUsageStats,
        thinking_content: Option<&str>,
        thinking_elapsed_ms: u64,
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role,
            content,
            segments_json: None,
            thinking_content,
            thinking_elapsed_ms,
            context_payload: None,
            tool_call_id: None,
            tool_name: None,
            tool_result_mode: None,
            tool_calls: None,
            tool_artifacts: &[],
            usage_stats: Some(usage_stats),
            visible: true,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_visible_message_with_tools(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_result_mode: Option<&str>,
        tool_calls: Option<&[OutboundToolCall]>,
    ) -> Result<DispatcherMessageRecord> {
        self.add_visible_message_with_tools_and_thinking(
            workspace_id,
            role,
            content,
            tool_call_id,
            tool_name,
            tool_result_mode,
            tool_calls,
            None,
            0,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_visible_message_with_tools_and_thinking(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_result_mode: Option<&str>,
        tool_calls: Option<&[OutboundToolCall]>,
        thinking_content: Option<&str>,
        thinking_elapsed_ms: u64,
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role,
            content,
            segments_json: None,
            thinking_content,
            thinking_elapsed_ms,
            context_payload: None,
            tool_call_id,
            tool_name,
            tool_result_mode,
            tool_calls,
            tool_artifacts: &[],
            usage_stats: None,
            visible: true,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_visible_tool_result(
        &self,
        workspace_id: &str,
        content: &str,
        context_payload: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_result_mode: Option<&str>,
        tool_artifacts: &[ToolArtifactDraft],
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role: "tool",
            content,
            segments_json: None,
            thinking_content: None,
            thinking_elapsed_ms: 0,
            context_payload: Some(context_payload),
            tool_call_id,
            tool_name,
            tool_result_mode,
            tool_calls: None,
            tool_artifacts,
            usage_stats: None,
            visible: true,
        })
    }

    fn add_message(&self, params: NewDispatcherMessage<'_>) -> Result<DispatcherMessageRecord> {
        let tool_calls_json = params
            .tool_calls
            .map(serde_json::to_string)
            .transpose()
            .context("serialize tool calls")?;
        let usage_stats_json = params
            .usage_stats
            .map(serde_json::to_string)
            .transpose()
            .context("serialize dispatcher message usage stats")?;

        let segments_json = params
            .segments_json
            .unwrap_or_else(|| content_to_segments_json(params.content));
        let segments = try_parse_segments_json(&segments_json)?;

        let mut record = DispatcherMessageRecord {
            id: Uuid::new_v4().to_string(),
            workspace_id: params.workspace_id.to_string(),
            role: params.role.to_string(),
            segments_json,
            thinking_content: params
                .thinking_content
                .filter(|content| !content.trim().is_empty())
                .map(|s| s.to_string()),
            thinking_elapsed_ms: params
                .thinking_content
                .filter(|content| !content.trim().is_empty())
                .map(|_| params.thinking_elapsed_ms),
            context_payload: params.context_payload.map(|s| s.to_string()),
            tool_call_id: params.tool_call_id.map(|s| s.to_string()),
            tool_name: params.tool_name.map(|s| s.to_string()),
            tool_result_mode: params.tool_result_mode.map(|s| s.to_string()),
            tool_artifacts: Vec::new(),
            tool_calls_json: tool_calls_json.clone(),
            usage_stats: params.usage_stats.cloned(),
            created_at: now(),
        };

        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE dispatcher_sessions SET updated_at = ?1 WHERE id = ?2",
            params![&record.created_at, &record.workspace_id],
        )?;
        tx.execute(
            "UPDATE chat_sessions SET updated_at = ?1 WHERE id = ?2",
            params![&record.created_at, &record.workspace_id],
        )?;
        tx.execute(
            "UPDATE project_sessions SET updated_at = ?1 WHERE id = ?2",
            params![&record.created_at, &record.workspace_id],
        )?;

        let thinking_elapsed_ms = record
            .thinking_elapsed_ms
            .map(i64::try_from)
            .transpose()
            .context("convert thinking elapsed milliseconds for sqlite")?;

        tx.execute(
            "INSERT INTO dispatcher_messages (
                id, workspace_id, role, segments_json, thinking_content, thinking_elapsed_ms, context_payload, tool_call_id, tool_name, tool_result_mode, tool_artifacts_json, tool_calls_json, usage_stats_json, visible, created_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                &record.id,
                &record.workspace_id,
                &record.role,
                &record.segments_json,
                &record.thinking_content,
                &thinking_elapsed_ms,
                &record.context_payload,
                &record.tool_call_id,
                &record.tool_name,
                &record.tool_result_mode,
                Option::<String>::None,
                &record.tool_calls_json,
                &usage_stats_json,
                if params.visible { 1 } else { 0 },
                &record.created_at
            ],
        )
        .context("insert dispatcher message")?;

        insert_chat_images(
            &tx,
            &record.workspace_id,
            &record.id,
            &segments,
            &record.created_at,
        )?;

        if !params.tool_artifacts.is_empty() {
            let artifacts = self.insert_tool_artifacts(
                &tx,
                &record.workspace_id,
                &record.id,
                record.tool_call_id.as_deref(),
                record.tool_name.as_deref(),
                params.tool_artifacts,
                &record.created_at,
            )?;
            let artifacts_json =
                serde_json::to_string(&artifacts).context("serialize dispatcher tool artifacts")?;
            tx.execute(
                "UPDATE dispatcher_messages SET tool_artifacts_json = ?1 WHERE id = ?2",
                params![&artifacts_json, &record.id],
            )
            .context("attach dispatcher tool artifacts to message")?;
            record.tool_artifacts = artifacts;
        }

        tx.commit().context("commit dispatcher message insert")?;

        Ok(record)
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_tool_artifacts(
        &self,
        tx: &rusqlite::Transaction<'_>,
        workspace_id: &str,
        message_id: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        drafts: &[ToolArtifactDraft],
        created_at: &str,
    ) -> Result<Vec<DispatcherToolArtifactRef>> {
        let mut refs = Vec::with_capacity(drafts.len());
        let tool_run_id = match tool_call_id {
            Some(tool_call_id) => tx
                .query_row(
                    "SELECT id FROM dispatcher_tool_runs
                     WHERE workspace_id = ?1 AND tool_call_id = ?2
                     ORDER BY created_at DESC, rowid DESC
                     LIMIT 1",
                    params![workspace_id, tool_call_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .context("resolve dispatcher tool run for artifact")?,
            None => None,
        };

        for draft in drafts {
            let artifact_id = Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO dispatcher_tool_artifacts (
                    id, workspace_id, message_id, tool_call_id, tool_run_id, tool_name,
                    title, kind, preview, content, char_count, line_count, created_at
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    &artifact_id,
                    workspace_id,
                    message_id,
                    tool_call_id,
                    &tool_run_id,
                    tool_name,
                    &draft.title,
                    &draft.kind,
                    &draft.preview,
                    &draft.content,
                    draft.char_count as i64,
                    draft.line_count as i64,
                    created_at,
                ],
            )
            .context("insert dispatcher tool artifact")?;

            refs.push(DispatcherToolArtifactRef {
                id: artifact_id,
                title: draft.title.clone(),
                kind: draft.kind.clone(),
                preview: draft.preview.clone(),
                char_count: draft.char_count,
                line_count: draft.line_count,
                created_at: created_at.to_string(),
            });
        }

        Ok(refs)
    }
}

// G9-05：LLM 上下文过滤的唯一实现位于 `crate::agent::common::should_keep_llm_message`。
// 本文件的 `load_llm_history` 与 `DispatcherMessageRecord::to_llm_message` 直接委托，
// 不再维护同口径的第二份私有过滤函数，消除双实现漂移风险。

#[cfg(test)]
mod tests;

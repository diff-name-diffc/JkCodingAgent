//! 消息（dispatcher_messages 表）的增删、历史加载、LLM 历史过滤，以及对应的
//! `spawn_blocking` 异步包装。私有核心插入逻辑（add_message / insert_tool_artifacts）
//! 亦在此处。

use std::collections::HashSet;

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent::llm::{ChatMessage, OutboundToolCall};

use super::artifacts::{DispatcherToolArtifactRef, ToolArtifactDraft};
use super::content::{
    content_to_segments_json, delete_chat_image_resources, delete_plan_file_resources,
    insert_chat_images, parse_segments_json, segments_to_markdown,
};
use super::util::{
    latest_user_message_rowid, map_dispatcher_message_record, now, MAX_LLM_DIALOGUES,
    TOOL_RETRY_CONTEXT_PREFIX,
};
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
    pub content: String,
    pub segments_json: String,
    pub thinking_content: Option<String>,
    pub thinking_elapsed_ms: Option<u64>,
    #[serde(skip_serializing)]
    pub context_payload: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_result_mode: Option<String>,
    pub tool_artifacts: Vec<DispatcherToolArtifactRef>,
    pub tool_calls_json: Option<String>,
    pub usage_stats: Option<DispatcherMessageUsageStats>,
    pub created_at: String,
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
    pub fn add_visible_message(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        segments_json: Option<String>,
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role,
            content,
            segments_json,
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

    pub fn compact_successful_tool_retry(
        &self,
        workspace_id: &str,
        tool_name: &str,
        current_tool_call_id: &str,
    ) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let cutoff_rowid = latest_user_message_rowid(&tx, workspace_id)?;
        let retry_context_pattern = format!("{TOOL_RETRY_CONTEXT_PREFIX}%");
        let retry_messages = {
            let mut stmt = tx.prepare(
                "SELECT id, tool_call_id
                 FROM dispatcher_messages
                 WHERE workspace_id = ?1
                   AND role = 'tool'
                   AND tool_name = ?2
                   AND rowid >= ?3
                   AND tool_call_id IS NOT NULL
                   AND tool_call_id <> ?4
                   AND context_payload LIKE ?5",
            )?;
            let rows = stmt.query_map(
                params![
                    workspace_id,
                    tool_name,
                    cutoff_rowid,
                    current_tool_call_id,
                    retry_context_pattern
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        if retry_messages.is_empty() {
            tx.commit()
                .context("commit empty dispatcher retry compaction")?;
            return Ok(());
        }

        let failed_tool_call_ids = retry_messages
            .iter()
            .map(|(_, tool_call_id)| tool_call_id.clone())
            .collect::<HashSet<_>>();

        for (message_id, _) in &retry_messages {
            tx.execute(
                "DELETE FROM dispatcher_tool_artifacts WHERE message_id = ?1",
                params![message_id],
            )
            .context("delete compacted retry tool artifacts")?;
            tx.execute(
                "DELETE FROM dispatcher_messages WHERE id = ?1",
                params![message_id],
            )
            .context("delete compacted retry tool message")?;
        }

        let assistant_messages = {
            let mut stmt = tx.prepare(
                "SELECT id, segments_json, tool_calls_json
                 FROM dispatcher_messages
                 WHERE workspace_id = ?1
                   AND role = 'assistant'
                   AND rowid >= ?2
                   AND tool_calls_json IS NOT NULL
                 ORDER BY rowid ASC",
            )?;
            let rows = stmt.query_map(params![workspace_id, cutoff_rowid], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        for (message_id, segments_json, tool_calls_json) in assistant_messages {
            let Ok(mut tool_calls) =
                serde_json::from_str::<Vec<OutboundToolCall>>(&tool_calls_json)
            else {
                continue;
            };
            let original_len = tool_calls.len();
            tool_calls.retain(|call| !failed_tool_call_ids.contains(&call.id));
            if tool_calls.len() == original_len {
                continue;
            }

            let content = segments_to_markdown(&parse_segments_json(&segments_json));
            if tool_calls.is_empty() && content.trim().is_empty() {
                tx.execute(
                    "DELETE FROM dispatcher_tool_artifacts WHERE message_id = ?1",
                    params![&message_id],
                )
                .context("delete compacted retry assistant artifacts")?;
                tx.execute(
                    "DELETE FROM dispatcher_messages WHERE id = ?1",
                    params![&message_id],
                )
                .context("delete compacted retry assistant message")?;
            } else {
                let next_tool_calls_json = if tool_calls.is_empty() {
                    None
                } else {
                    Some(
                        serde_json::to_string(&tool_calls)
                            .context("serialize compacted assistant tool calls")?,
                    )
                };
                tx.execute(
                    "UPDATE dispatcher_messages SET tool_calls_json = ?1 WHERE id = ?2",
                    params![next_tool_calls_json, &message_id],
                )
                .context("update compacted assistant tool calls")?;
            }
        }

        tx.commit()
            .context("commit dispatcher successful retry compaction")
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_hidden_message(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_result_mode: Option<&str>,
        tool_calls: Option<&[OutboundToolCall]>,
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role,
            content,
            segments_json: None,
            thinking_content: None,
            thinking_elapsed_ms: 0,
            context_payload: None,
            tool_call_id,
            tool_name,
            tool_result_mode,
            tool_calls,
            tool_artifacts: &[],
            usage_stats: None,
            visible: false,
        })
    }

    pub fn list_visible_messages(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<DispatcherMessageRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, role, segments_json, thinking_content, thinking_elapsed_ms, context_payload, tool_call_id, tool_name, tool_result_mode, tool_artifacts_json, tool_calls_json, usage_stats_json, created_at
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND visible = 1
             ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = stmt.query_map(params![workspace_id], map_dispatcher_message_record)?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("load visible dispatcher messages")
    }

    /// Load recent complete visible dialogues for session title generation.
    ///
    /// The cutoff is based on user-started turns, so the latest user message and its
    /// following assistant/tool messages stay together instead of being clipped by a
    /// raw message count.
    pub fn list_recent_visible_dialogue_messages(
        &self,
        workspace_id: &str,
        max_dialogues: usize,
    ) -> Result<Vec<DispatcherMessageRecord>> {
        let conn = self.conn()?;
        let cutoff_rowid = self.find_dialogue_cutoff_rowid(&conn, workspace_id, max_dialogues)?;
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, role, segments_json, thinking_content, thinking_elapsed_ms, context_payload, tool_call_id, tool_name, tool_result_mode, tool_artifacts_json, tool_calls_json, usage_stats_json, created_at
             FROM dispatcher_messages
             WHERE workspace_id = ?1
               AND visible = 1
               AND context_cleared = 0
               AND rowid >= ?2
             ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = stmt.query_map(
            params![workspace_id, cutoff_rowid],
            map_dispatcher_message_record,
        )?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("load recent visible dispatcher dialogue messages")
    }

    /// Load only the recent dialogue window for one dispatcher session.
    ///
    /// Note:
    /// - `workspace_id` here is the dispatcher session id used by the frontend.
    /// - One project can have multiple dispatcher sessions; history is isolated by session id.
    /// - Only the most recent `MAX_LLM_DIALOGUES` user-started dialogues are injected into the LLM.
    pub fn load_llm_history(&self, workspace_id: &str) -> Result<Vec<ChatMessage>> {
        let conn = self.conn()?;
        let cutoff_rowid =
            self.find_dialogue_cutoff_rowid(&conn, workspace_id, MAX_LLM_DIALOGUES)?;

        let mut stmt = conn.prepare(
            "SELECT role, segments_json, context_payload, tool_call_id, tool_name, tool_calls_json, thinking_content
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND rowid >= ?2 AND context_cleared = 0
             ORDER BY rowid ASC",
        )?;
        let rows = stmt.query_map(params![workspace_id, cutoff_rowid], |row| {
            let role: String = row.get(0)?;
            let segments_json: String = row.get(1)?;
            let context_payload: Option<String> = row.get(2)?;
            let tool_call_id: Option<String> = row.get(3)?;
            let tool_name: Option<String> = row.get(4)?;
            let tool_calls_json: Option<String> = row.get(5)?;
            let thinking_content: Option<String> = row.get(6)?;

            let content = if let Some(payload) = context_payload {
                payload
            } else {
                let segments = parse_segments_json(&segments_json);
                segments_to_markdown(&segments)
            };

            let tool_calls = tool_calls_json
                .as_deref()
                .and_then(|json| serde_json::from_str::<Vec<OutboundToolCall>>(json).ok());

            Ok(ChatMessage {
                reasoning_content: if role == "assistant" {
                    thinking_content.filter(|content| !content.trim().is_empty())
                } else {
                    None
                },
                role,
                content,
                tool_call_id,
                name: tool_name,
                tool_calls,
            })
        })?;

        let mut messages = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("load dispatcher llm history")?;
        messages.retain(should_keep_llm_message);

        while matches!(messages.first().map(|m| m.role.as_str()), Some("tool")) {
            messages.remove(0);
        }

        Ok(messages)
    }

    /// Fetch only the content of the latest visible user message.
    ///
    /// Lighter than `load_llm_history` — skips parsing tool calls, context payloads,
    /// and dialogue cutoff calculations.
    pub fn get_latest_user_message_content(&self, workspace_id: &str) -> Result<Option<String>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT segments_json
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND role = 'user' AND visible = 1 AND context_cleared = 0
             ORDER BY rowid DESC
             LIMIT 1",
            params![workspace_id],
            |row| {
                let segments_json: String = row.get(0)?;
                Ok(segments_to_markdown(&parse_segments_json(&segments_json)))
            },
        )
        .optional()
        .context("load latest dispatcher user message content")
    }

    pub fn get_visible_message_content(
        &self,
        workspace_id: &str,
        message_id: &str,
    ) -> Result<Option<String>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT segments_json
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND id = ?2 AND visible = 1
             LIMIT 1",
            params![workspace_id, message_id],
            |row| {
                let segments_json: String = row.get(0)?;
                Ok(segments_to_markdown(&parse_segments_json(&segments_json)))
            },
        )
        .optional()
        .context("load dispatcher visible message content")
    }

    /// Fetch recent tool/assistant message content for subprocess context.
    ///
    /// Returns up to `limit` recent non-user messages (tool results and assistant replies),
    /// each as (role, tool_name, content) tuples — enough to build exploration context
    /// without loading the full LLM history.
    pub fn list_recent_exploration_content(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<(String, Option<String>, String)>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT role, tool_name, segments_json
             FROM dispatcher_messages
             WHERE workspace_id = ?1
               AND role IN ('tool', 'assistant')
               AND visible = 1
               AND context_cleared = 0
             ORDER BY rowid DESC
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![workspace_id, limit as i64], |row| {
                let role: String = row.get(0)?;
                let tool_name: Option<String> = row.get(1)?;
                let segments_json: String = row.get(2)?;
                let content = segments_to_markdown(&parse_segments_json(&segments_json));
                Ok((role, tool_name, content))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("load recent dispatcher exploration content")?;
        Ok(rows)
    }

    pub fn clear_context_messages(&self, workspace_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE dispatcher_messages
             SET context_cleared = 1
             WHERE workspace_id = ?1 AND context_cleared = 0",
            params![workspace_id],
        )
        .context("logically clear dispatcher messages")?;
        Ok(())
    }

    pub fn clear_messages(&self, workspace_id: &str) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM dispatcher_tool_artifacts WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear dispatcher tool artifacts")?;
        tx.execute(
            "DELETE FROM dispatcher_session_token_usage WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear dispatcher session token usage")?;
        delete_chat_image_resources(&tx, workspace_id)?;
        delete_plan_file_resources(&tx, workspace_id)?;
        tx.execute(
            "DELETE FROM session_keywords WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear session keywords")?;
        tx.execute(
            "DELETE FROM dispatcher_messages WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear dispatcher messages")?;
        tx.execute(
            "UPDATE dispatcher_sessions
             SET checklist_json = NULL,
                 plan_interaction_json = NULL,
                 active_plan_path = NULL,
                 updated_at = ?1
             WHERE id = ?2",
            params![now(), workspace_id],
        )
        .context("clear dispatcher planning state")?;
        tx.execute(
            "UPDATE project_sessions
             SET active_plan_path = NULL,
                 checklist_json = NULL,
                 plan_interaction_json = NULL,
                 updated_at = ?1
             WHERE id = ?2",
            params![now(), workspace_id],
        )
        .context("clear project session planning state")?;
        tx.execute(
            "UPDATE chat_sessions SET updated_at = ?1 WHERE id = ?2",
            params![now(), workspace_id],
        )
        .context("update chat session updated_at after clear")?;
        tx.commit().context("commit dispatcher message cleanup")?;
        Ok(())
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
        let segments = parse_segments_json(&segments_json);
        let content = segments_to_markdown(&segments);

        let mut record = DispatcherMessageRecord {
            id: Uuid::new_v4().to_string(),
            workspace_id: params.workspace_id.to_string(),
            role: params.role.to_string(),
            content,
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

        for draft in drafts {
            let artifact_id = Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO dispatcher_tool_artifacts (
                    id, workspace_id, message_id, tool_call_id, tool_name, title, kind, preview, content, char_count, line_count, created_at
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    &artifact_id,
                    workspace_id,
                    message_id,
                    tool_call_id,
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
    pub async fn add_visible_message_async(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        segments_json: Option<String>,
    ) -> Result<DispatcherMessageRecord> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        let role = role.to_string();
        let content = content.to_string();
        tokio::task::spawn_blocking(move || {
            db.add_visible_message(&wid, &role, &content, segments_json)
        })
        .await
        .context("add_visible_message spawn_blocking")?
    }

    pub async fn add_visible_message_with_usage_and_thinking_async(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        usage_stats: &DispatcherMessageUsageStats,
        thinking_content: Option<&str>,
        thinking_elapsed_ms: u64,
    ) -> Result<DispatcherMessageRecord> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        let role = role.to_string();
        let content = content.to_string();
        let usage = usage_stats.clone();
        let thinking = thinking_content.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            db.add_visible_message_with_usage_and_thinking(
                &wid,
                &role,
                &content,
                &usage,
                thinking.as_deref(),
                thinking_elapsed_ms,
            )
        })
        .await
        .context("add_visible_message_with_usage_and_thinking spawn_blocking")?
    }

    pub async fn load_llm_history_async(&self, workspace_id: &str) -> Result<Vec<ChatMessage>> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        tokio::task::spawn_blocking(move || db.load_llm_history(&wid))
            .await
            .context("load_llm_history spawn_blocking")?
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn add_visible_message_with_tools_and_thinking_async(
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
        let db = self.clone();
        let wid = workspace_id.to_string();
        let role = role.to_string();
        let content = content.to_string();
        let tool_call_id = tool_call_id.map(str::to_string);
        let tool_name = tool_name.map(str::to_string);
        let tool_result_mode = tool_result_mode.map(str::to_string);
        let tool_calls = tool_calls.map(|c| c.to_vec());
        let thinking = thinking_content.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            db.add_visible_message_with_tools_and_thinking(
                &wid,
                &role,
                &content,
                tool_call_id.as_deref(),
                tool_name.as_deref(),
                tool_result_mode.as_deref(),
                tool_calls.as_deref(),
                thinking.as_deref(),
                thinking_elapsed_ms,
            )
        })
        .await
        .context("add_visible_message_with_tools_and_thinking spawn_blocking")?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn add_visible_tool_result_async(
        &self,
        workspace_id: &str,
        content: &str,
        context_payload: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_result_mode: Option<&str>,
        tool_artifacts: &[ToolArtifactDraft],
    ) -> Result<DispatcherMessageRecord> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        let content = content.to_string();
        let context_payload = context_payload.to_string();
        let tool_call_id = tool_call_id.map(str::to_string);
        let tool_name = tool_name.map(str::to_string);
        let tool_result_mode = tool_result_mode.map(str::to_string);
        let artifacts = tool_artifacts.to_vec();
        tokio::task::spawn_blocking(move || {
            db.add_visible_tool_result(
                &wid,
                &content,
                &context_payload,
                tool_call_id.as_deref(),
                tool_name.as_deref(),
                tool_result_mode.as_deref(),
                &artifacts,
            )
        })
        .await
        .context("add_visible_tool_result spawn_blocking")?
    }

    pub async fn list_visible_messages_async(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<DispatcherMessageRecord>> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        tokio::task::spawn_blocking(move || db.list_visible_messages(&wid))
            .await
            .context("list_visible_messages spawn_blocking")?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn add_hidden_message_async(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_result_mode: Option<&str>,
        tool_calls: Option<&[OutboundToolCall]>,
    ) -> Result<DispatcherMessageRecord> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        let role = role.to_string();
        let content = content.to_string();
        let tool_call_id = tool_call_id.map(str::to_string);
        let tool_name = tool_name.map(str::to_string);
        let tool_result_mode = tool_result_mode.map(str::to_string);
        let tool_calls = tool_calls.map(|c| c.to_vec());
        tokio::task::spawn_blocking(move || {
            db.add_hidden_message(
                &wid,
                &role,
                &content,
                tool_call_id.as_deref(),
                tool_name.as_deref(),
                tool_result_mode.as_deref(),
                tool_calls.as_deref(),
            )
        })
        .await
        .context("add_hidden_message spawn_blocking")?
    }

    pub async fn add_visible_message_with_usage_async(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        usage_stats: &DispatcherMessageUsageStats,
    ) -> Result<DispatcherMessageRecord> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        let role = role.to_string();
        let content = content.to_string();
        let usage_stats = usage_stats.clone();
        tokio::task::spawn_blocking(move || {
            db.add_visible_message_with_usage(&wid, &role, &content, &usage_stats)
        })
        .await
        .context("add_visible_message_with_usage spawn_blocking")?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn add_visible_message_with_tools_async(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_result_mode: Option<&str>,
        tool_calls: Option<&[OutboundToolCall]>,
    ) -> Result<DispatcherMessageRecord> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        let role = role.to_string();
        let content = content.to_string();
        let tool_call_id = tool_call_id.map(str::to_string);
        let tool_name = tool_name.map(str::to_string);
        let tool_result_mode = tool_result_mode.map(str::to_string);
        let tool_calls = tool_calls.map(|c| c.to_vec());
        tokio::task::spawn_blocking(move || {
            db.add_visible_message_with_tools(
                &wid,
                &role,
                &content,
                tool_call_id.as_deref(),
                tool_name.as_deref(),
                tool_result_mode.as_deref(),
                tool_calls.as_deref(),
            )
        })
        .await
        .context("add_visible_message_with_tools spawn_blocking")?
    }
    pub async fn compact_successful_tool_retry_async(
        &self,
        workspace_id: &str,
        tool_name: &str,
        current_tool_call_id: &str,
    ) -> Result<()> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        let tool_name = tool_name.to_string();
        let tool_call_id = current_tool_call_id.to_string();
        tokio::task::spawn_blocking(move || {
            db.compact_successful_tool_retry(&wid, &tool_name, &tool_call_id)
        })
        .await
        .context("compact_successful_tool_retry spawn_blocking")?
    }

    pub async fn get_latest_user_message_content_async(
        &self,
        workspace_id: &str,
    ) -> Result<Option<String>> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        tokio::task::spawn_blocking(move || db.get_latest_user_message_content(&wid))
            .await
            .context("get_latest_user_message_content spawn_blocking")?
    }

    pub async fn list_recent_exploration_content_async(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<(String, Option<String>, String)>> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        tokio::task::spawn_blocking(move || db.list_recent_exploration_content(&wid, limit))
            .await
            .context("list_recent_exploration_content spawn_blocking")?
    }
}

fn should_keep_llm_message(message: &ChatMessage) -> bool {
    match message.role.as_str() {
        "assistant" => {
            !is_process_only_assistant_message(&message.content)
                && !is_process_only_assistant_tool_call(message)
        }
        "tool" => !message
            .name
            .as_deref()
            .is_some_and(is_dispatch_plumbing_tool_name),
        _ => true,
    }
}

fn is_process_only_assistant_message(content: &str) -> bool {
    let trimmed = content.trim();
    matches!(
        trimmed,
        "🔄 子任务当前轮次已完成"
            | "✅ 子任务进程已结束"
            | "⚠️ 子任务进程已失败退出"
            | "⏹️ 子任务进程已取消"
            | "🔄 子任务当前轮次已完成，执行结果已同步供后续分析。"
            | "✅ 子任务进程已结束，执行结果已同步供后续分析。"
            | "⚠️ 子任务进程已失败退出，执行结果已同步供后续分析。"
            | "⏹️ 子任务进程已取消，执行结果已同步供后续分析。"
    ) || trimmed.starts_with("📋 已自动批准 ")
        || content.starts_with("📋 已提交 ")
        || content.starts_with("📨 已向 ")
        || content.starts_with("⏹️ 已向 ")
}

fn is_process_only_assistant_tool_call(message: &ChatMessage) -> bool {
    message
        .tool_calls
        .as_ref()
        .is_some_and(|calls| !calls.is_empty() && calls.iter().all(is_dispatch_plumbing_tool_call))
}

fn is_dispatch_plumbing_tool_call(call: &OutboundToolCall) -> bool {
    is_dispatch_plumbing_tool_name(&call.function.name)
}

fn is_dispatch_plumbing_tool_name(name: &str) -> bool {
    matches!(
        name,
        "dispatch_claude"
            | "dispatch_codex"
            | "continue_claude_session"
            | "continue_codex_session"
            | "exit_claude_session"
            | "exit_codex_session"
    )
}

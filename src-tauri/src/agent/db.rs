use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::config::DEFAULT_SUMMARY_MODEL;
use super::llm::{ChatMessage, LlmUsage, OutboundToolCall};
use super::summary::ToolArtifactDraft;

const MAX_LLM_DIALOGUES: usize = 5;
const DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS: u64 = 1_000_000;
pub(crate) const TOOL_RETRY_CONTEXT_PREFIX: &str = "[工具调用失败，已交回模型修正重试]";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherSessionRecord {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherSettingsRecord {
    pub api_base: String,
    pub api_key: String,
    pub model: String,
    pub summary_model: String,
    pub vision_model: String,
    pub asr_api_key: String,
    pub asr_websocket_url: String,
    pub auto_approve_dispatch: bool,
    pub context_debug: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherMessageRecord {
    pub id: String,
    pub workspace_id: String,
    pub role: String,
    pub content: String,
    #[serde(skip_serializing)]
    pub context_payload: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_result_mode: Option<String>,
    pub tool_artifacts: Vec<DispatcherToolArtifactRef>,
    pub tool_calls_json: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherToolArtifactRef {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub preview: String,
    pub char_count: usize,
    pub line_count: usize,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherToolArtifactRecord {
    pub id: String,
    pub workspace_id: String,
    pub message_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub title: String,
    pub kind: String,
    pub preview: String,
    pub content: String,
    pub char_count: usize,
    pub line_count: usize,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DispatcherSessionTokenUsageSource {
    Primary,
    Summary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherSessionTokenUsageRecord {
    pub workspace_id: String,
    pub model: String,
    pub source_kind: DispatcherSessionTokenUsageSource,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub cached_tokens: u64,
    pub context_window_tokens: u64,
    pub context_window_capacity: u64,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct DispatcherDb {
    path: PathBuf,
}

struct NewDispatcherMessage<'a> {
    workspace_id: &'a str,
    role: &'a str,
    content: &'a str,
    context_payload: Option<&'a str>,
    tool_call_id: Option<&'a str>,
    tool_name: Option<&'a str>,
    tool_result_mode: Option<&'a str>,
    tool_calls: Option<&'a [OutboundToolCall]>,
    tool_artifacts: &'a [ToolArtifactDraft],
    visible: bool,
}

impl DispatcherDb {
    pub fn new(path: PathBuf) -> Result<Self> {
        let db = Self { path };
        db.init()?;
        Ok(db)
    }

    // ── Settings ──────────────────────────────────────────────

    pub fn get_settings(&self) -> Result<Option<DispatcherSettingsRecord>> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT api_base, api_key, model, summary_model, vision_model, asr_api_key, asr_websocket_url, auto_approve_dispatch, context_debug FROM dispatcher_settings WHERE id = 'default'",
            [],
            |row| {
                Ok(DispatcherSettingsRecord {
                    api_base: row.get(0)?,
                    api_key: row.get(1)?,
                    model: row.get(2)?,
                    summary_model: row.get(3)?,
                    vision_model: row.get(4)?,
                    asr_api_key: row.get(5)?,
                    asr_websocket_url: row.get(6)?,
                    auto_approve_dispatch: row.get::<_, i32>(7)? != 0,
                    context_debug: row.get::<_, i32>(8)? != 0,
                })
            },
        )
        .optional()
        .context("load dispatcher settings")
    }

    pub fn save_settings(
        &self,
        api_base: &str,
        api_key: &str,
        model: &str,
        summary_model: &str,
        vision_model: &str,
        asr_api_key: &str,
        asr_websocket_url: &str,
        auto_approve_dispatch: bool,
        context_debug: bool,
    ) -> Result<DispatcherSettingsRecord> {
        let conn = self.connect()?;
        let auto_approve_int = if auto_approve_dispatch { 1 } else { 0 };
        let context_debug_int = if context_debug { 1 } else { 0 };
        let normalized_summary_model = normalize_summary_model(summary_model);
        conn.execute(
            "INSERT INTO dispatcher_settings (id, api_base, api_key, model, summary_model, vision_model, asr_api_key, asr_websocket_url, auto_approve_dispatch, context_debug)
             VALUES ('default', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET api_base = ?1, api_key = ?2, model = ?3, summary_model = ?4, vision_model = ?5, asr_api_key = ?6, asr_websocket_url = ?7, auto_approve_dispatch = ?8, context_debug = ?9",
            params![
                api_base.trim(),
                api_key.trim(),
                model.trim(),
                &normalized_summary_model,
                vision_model.trim(),
                asr_api_key.trim(),
                asr_websocket_url.trim(),
                auto_approve_int,
                context_debug_int
            ],
        )
        .context("save dispatcher settings")?;
        Ok(DispatcherSettingsRecord {
            api_base: api_base.trim().to_string(),
            api_key: api_key.trim().to_string(),
            model: model.trim().to_string(),
            summary_model: normalized_summary_model,
            vision_model: vision_model.trim().to_string(),
            asr_api_key: asr_api_key.trim().to_string(),
            asr_websocket_url: asr_websocket_url.trim().to_string(),
            auto_approve_dispatch,
            context_debug,
        })
    }

    pub fn set_auto_approve_dispatch(
        &self,
        auto_approve_dispatch: bool,
    ) -> Result<DispatcherSettingsRecord> {
        let conn = self.connect()?;
        let auto_approve_int = if auto_approve_dispatch { 1 } else { 0 };
        conn.execute(
            "INSERT INTO dispatcher_settings (id, auto_approve_dispatch)
             VALUES ('default', ?1)
             ON CONFLICT(id) DO UPDATE SET auto_approve_dispatch = ?1",
            params![auto_approve_int],
        )
        .context("save dispatcher auto-approve setting")?;
        self.get_settings()?
            .context("load dispatcher settings after auto-approve update")
    }

    // ── Sessions ──────────────────────────────────────────────

    pub fn list_sessions(&self, project_id: &str) -> Result<Vec<DispatcherSessionRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, project_id, title, created_at, updated_at
             FROM dispatcher_sessions
             WHERE project_id = ?1
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok(DispatcherSessionRecord {
                id: row.get(0)?,
                project_id: row.get(1)?,
                title: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("list dispatcher sessions")
    }

    pub fn create_session(&self, project_id: &str, title: &str) -> Result<DispatcherSessionRecord> {
        let record = DispatcherSessionRecord {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_string(),
            title: title.to_string(),
            created_at: now(),
            updated_at: now(),
        };

        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO dispatcher_sessions (id, project_id, title, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                record.id,
                record.project_id,
                record.title,
                record.created_at,
                record.updated_at
            ],
        )
        .context("insert dispatcher session")?;

        Ok(record)
    }

    pub fn update_session_title(
        &self,
        session_id: &str,
        title: &str,
    ) -> Result<Option<DispatcherSessionRecord>> {
        let updated_at = now();
        let conn = self.connect()?;
        let changed = conn
            .execute(
                "UPDATE dispatcher_sessions
                 SET title = ?1, updated_at = ?2
                 WHERE id = ?3",
                params![title.trim(), &updated_at, session_id],
            )
            .context("update dispatcher session title")?;

        if changed == 0 {
            return Ok(None);
        }

        conn.query_row(
            "SELECT id, project_id, title, created_at, updated_at
             FROM dispatcher_sessions
             WHERE id = ?1",
            params![session_id],
            |row| {
                Ok(DispatcherSessionRecord {
                    id: row.get(0)?,
                    project_id: row.get(1)?,
                    title: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            },
        )
        .optional()
        .context("load dispatcher session after title update")
    }

    pub fn delete_session(&self, session_id: &str) -> Result<()> {
        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM dispatcher_tool_artifacts WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_session_token_usage WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_messages WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_sessions WHERE id = ?1",
            params![session_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    // ── Messages ──────────────────────────────────────────────

    pub fn add_visible_message(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role,
            content,
            context_payload: None,
            tool_call_id: None,
            tool_name: None,
            tool_result_mode: None,
            tool_calls: None,
            tool_artifacts: &[],
            visible: true,
        })
    }

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
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role,
            content,
            context_payload: None,
            tool_call_id,
            tool_name,
            tool_result_mode,
            tool_calls,
            tool_artifacts: &[],
            visible: true,
        })
    }

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
            context_payload: Some(context_payload),
            tool_call_id,
            tool_name,
            tool_result_mode,
            tool_calls: None,
            tool_artifacts,
            visible: true,
        })
    }

    pub fn compact_successful_tool_retry(
        &self,
        workspace_id: &str,
        tool_name: &str,
        current_tool_call_id: &str,
    ) -> Result<()> {
        let mut conn = self.connect()?;
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
                "SELECT id, content, tool_calls_json
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

        for (message_id, content, tool_calls_json) in assistant_messages {
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
            context_payload: None,
            tool_call_id,
            tool_name,
            tool_result_mode,
            tool_calls,
            tool_artifacts: &[],
            visible: false,
        })
    }

    pub fn list_visible_messages(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<DispatcherMessageRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, role, content, context_payload, tool_call_id, tool_name, tool_result_mode, tool_artifacts_json, tool_calls_json, created_at
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND visible = 1
             ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = stmt.query_map(params![workspace_id], |row| {
            Ok(DispatcherMessageRecord {
                id: row.get(0)?,
                workspace_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                context_payload: row.get(4)?,
                tool_call_id: row.get(5)?,
                tool_name: row.get(6)?,
                tool_result_mode: row.get(7)?,
                tool_artifacts: parse_tool_artifact_refs(row.get::<_, Option<String>>(8)?),
                tool_calls_json: row.get(9)?,
                created_at: row.get(10)?,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("load visible dispatcher messages")
    }

    /// Load only the recent dialogue window for one dispatcher session.
    ///
    /// Note:
    /// - `workspace_id` here is the dispatcher session id used by the frontend.
    /// - One project can have multiple dispatcher sessions; history is isolated by session id.
    /// - Only the most recent `MAX_LLM_DIALOGUES` user-started dialogues are injected into the LLM.
    pub fn load_llm_history(&self, workspace_id: &str) -> Result<Vec<ChatMessage>> {
        let conn = self.connect()?;
        let cutoff_rowid =
            self.find_dialogue_cutoff_rowid(&conn, workspace_id, MAX_LLM_DIALOGUES)?;

        let mut stmt = conn.prepare(
            "SELECT role, COALESCE(context_payload, content), tool_call_id, tool_name, tool_calls_json
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND rowid >= ?2
             ORDER BY rowid ASC",
        )?;
        let rows = stmt.query_map(params![workspace_id, cutoff_rowid], map_chat_message_row)?;

        let mut messages = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("load dispatcher llm history")?;
        messages.retain(should_keep_llm_message);

        while matches!(messages.first().map(|m| m.role.as_str()), Some("tool")) {
            messages.remove(0);
        }

        Ok(messages)
    }

    pub fn clear_messages(&self, workspace_id: &str) -> Result<()> {
        let mut conn = self.connect()?;
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
        tx.execute(
            "DELETE FROM dispatcher_messages WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear dispatcher messages")?;
        tx.commit().context("commit dispatcher message cleanup")?;
        Ok(())
    }

    pub fn upsert_session_token_usage(
        &self,
        workspace_id: &str,
        model: &str,
        source_kind: DispatcherSessionTokenUsageSource,
        usage: &LlmUsage,
    ) -> Result<DispatcherSessionTokenUsageRecord> {
        let record = DispatcherSessionTokenUsageRecord {
            workspace_id: workspace_id.to_string(),
            model: model.to_string(),
            source_kind,
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
            cached_tokens: usage.cached_tokens(),
            context_window_tokens: usage.prompt_tokens,
            context_window_capacity: default_context_window_capacity(model),
            updated_at: now(),
        };
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO dispatcher_session_token_usage (
                workspace_id, model, source_kind, prompt_tokens, completion_tokens, total_tokens,
                cached_tokens, context_window_tokens, context_window_capacity, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(workspace_id, model) DO UPDATE SET
                source_kind = excluded.source_kind,
                prompt_tokens = excluded.prompt_tokens,
                completion_tokens = excluded.completion_tokens,
                total_tokens = excluded.total_tokens,
                cached_tokens = excluded.cached_tokens,
                context_window_tokens = excluded.context_window_tokens,
                context_window_capacity = excluded.context_window_capacity,
                updated_at = excluded.updated_at",
            params![
                &record.workspace_id,
                &record.model,
                source_kind.as_sql_value(),
                record.prompt_tokens as i64,
                record.completion_tokens as i64,
                record.total_tokens as i64,
                record.cached_tokens as i64,
                record.context_window_tokens as i64,
                record.context_window_capacity as i64,
                &record.updated_at,
            ],
        )
        .context("upsert dispatcher session token usage")?;
        Ok(record)
    }

    pub fn list_session_token_usage(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<DispatcherSessionTokenUsageRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT workspace_id, model, source_kind, prompt_tokens, completion_tokens, total_tokens,
                    cached_tokens, context_window_tokens, context_window_capacity, updated_at
             FROM dispatcher_session_token_usage
             WHERE workspace_id = ?1
             ORDER BY updated_at DESC, model ASC",
        )?;
        let rows = stmt.query_map(params![workspace_id], |row| {
            Ok(DispatcherSessionTokenUsageRecord {
                workspace_id: row.get(0)?,
                model: row.get(1)?,
                source_kind: DispatcherSessionTokenUsageSource::from_sql_value(row.get(2)?),
                prompt_tokens: row.get::<_, i64>(3)? as u64,
                completion_tokens: row.get::<_, i64>(4)? as u64,
                total_tokens: row.get::<_, i64>(5)? as u64,
                cached_tokens: row.get::<_, i64>(6)? as u64,
                context_window_tokens: row.get::<_, i64>(7)? as u64,
                context_window_capacity: row.get::<_, i64>(8)? as u64,
                updated_at: row.get(9)?,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("list dispatcher session token usage")
    }

    pub fn get_tool_artifact(
        &self,
        workspace_id: &str,
        artifact_id: &str,
    ) -> Result<DispatcherToolArtifactRecord> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT id, workspace_id, message_id, tool_call_id, tool_name, title, kind, preview, content, char_count, line_count, created_at
             FROM dispatcher_tool_artifacts
             WHERE id = ?1 AND workspace_id = ?2",
            params![artifact_id, workspace_id],
            |row| {
                Ok(DispatcherToolArtifactRecord {
                    id: row.get(0)?,
                    workspace_id: row.get(1)?,
                    message_id: row.get(2)?,
                    tool_call_id: row.get(3)?,
                    tool_name: row.get(4)?,
                    title: row.get(5)?,
                    kind: row.get(6)?,
                    preview: row.get(7)?,
                    content: row.get(8)?,
                    char_count: row.get::<_, i64>(9)? as usize,
                    line_count: row.get::<_, i64>(10)? as usize,
                    created_at: row.get(11)?,
                })
            },
        )
        .optional()
        .context("load dispatcher tool artifact")?
        .with_context(|| format!("dispatcher tool artifact not found: {artifact_id}"))
    }

    fn add_message(&self, params: NewDispatcherMessage<'_>) -> Result<DispatcherMessageRecord> {
        let tool_calls_json = params
            .tool_calls
            .map(serde_json::to_string)
            .transpose()
            .context("serialize tool calls")?;

        let mut record = DispatcherMessageRecord {
            id: Uuid::new_v4().to_string(),
            workspace_id: params.workspace_id.to_string(),
            role: params.role.to_string(),
            content: params.content.to_string(),
            context_payload: params.context_payload.map(|s| s.to_string()),
            tool_call_id: params.tool_call_id.map(|s| s.to_string()),
            tool_name: params.tool_name.map(|s| s.to_string()),
            tool_result_mode: params.tool_result_mode.map(|s| s.to_string()),
            tool_artifacts: Vec::new(),
            tool_calls_json: tool_calls_json.clone(),
            created_at: now(),
        };

        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE dispatcher_sessions SET updated_at = ?1 WHERE id = ?2",
            params![&record.created_at, &record.workspace_id],
        )?;

        tx.execute(
            "INSERT INTO dispatcher_messages (
                id, workspace_id, role, content, context_payload, tool_call_id, tool_name, tool_result_mode, tool_artifacts_json, tool_calls_json, visible, created_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                &record.id,
                &record.workspace_id,
                &record.role,
                &record.content,
                &record.context_payload,
                &record.tool_call_id,
                &record.tool_name,
                &record.tool_result_mode,
                Option::<String>::None,
                &record.tool_calls_json,
                if params.visible { 1 } else { 0 },
                &record.created_at
            ],
        )
        .context("insert dispatcher message")?;

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

    fn init(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create db directory {}", parent.display()))?;
        }
        let conn = self.connect()?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS dispatcher_sessions (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                title TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_dispatcher_sessions_project
            ON dispatcher_sessions(project_id, updated_at DESC);

            CREATE TABLE IF NOT EXISTS dispatcher_messages (
                id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                context_payload TEXT,
                tool_call_id TEXT,
                tool_name TEXT,
                tool_result_mode TEXT,
                tool_artifacts_json TEXT,
                tool_calls_json TEXT,
                visible INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_dispatcher_messages_workspace_created
            ON dispatcher_messages(workspace_id, created_at);

            CREATE TABLE IF NOT EXISTS dispatcher_tool_artifacts (
                id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                message_id TEXT,
                tool_call_id TEXT,
                tool_name TEXT,
                title TEXT NOT NULL,
                kind TEXT NOT NULL,
                preview TEXT NOT NULL DEFAULT '',
                content TEXT NOT NULL,
                char_count INTEGER NOT NULL DEFAULT 0,
                line_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_dispatcher_tool_artifacts_workspace_created
            ON dispatcher_tool_artifacts(workspace_id, created_at);

            CREATE INDEX IF NOT EXISTS idx_dispatcher_tool_artifacts_message
            ON dispatcher_tool_artifacts(message_id);

            CREATE TABLE IF NOT EXISTS dispatcher_settings (
                id TEXT PRIMARY KEY DEFAULT 'default',
                api_base TEXT NOT NULL DEFAULT '',
                api_key TEXT NOT NULL DEFAULT '',
                model TEXT NOT NULL DEFAULT '',
                summary_model TEXT NOT NULL DEFAULT 'deepseek-v4-flash',
                vision_model TEXT NOT NULL DEFAULT '',
                asr_api_key TEXT NOT NULL DEFAULT '',
                asr_websocket_url TEXT NOT NULL DEFAULT '',
                auto_approve_dispatch INTEGER NOT NULL DEFAULT 0,
                context_debug INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS dispatcher_session_token_usage (
                workspace_id TEXT NOT NULL,
                model TEXT NOT NULL,
                source_kind TEXT NOT NULL DEFAULT 'primary',
                prompt_tokens INTEGER NOT NULL DEFAULT 0,
                completion_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                cached_tokens INTEGER NOT NULL DEFAULT 0,
                context_window_tokens INTEGER NOT NULL DEFAULT 0,
                context_window_capacity INTEGER NOT NULL DEFAULT 1000000,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (workspace_id, model)
            );

            CREATE INDEX IF NOT EXISTS idx_dispatcher_session_token_usage_workspace_updated
            ON dispatcher_session_token_usage(workspace_id, updated_at DESC);
            ",
        )
        .context("initialize dispatcher sqlite schema")?;
        ensure_column_exists(
            &conn,
            "dispatcher_settings",
            "summary_model",
            "TEXT NOT NULL DEFAULT 'deepseek-v4-flash'",
        )?;
        ensure_column_exists(
            &conn,
            "dispatcher_settings",
            "vision_model",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        ensure_column_exists(
            &conn,
            "dispatcher_settings",
            "asr_api_key",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        ensure_column_exists(
            &conn,
            "dispatcher_settings",
            "asr_websocket_url",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        ensure_column_exists(
            &conn,
            "dispatcher_settings",
            "context_debug",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        ensure_column_exists(&conn, "dispatcher_messages", "context_payload", "TEXT")?;
        ensure_column_exists(&conn, "dispatcher_messages", "tool_result_mode", "TEXT")?;
        ensure_column_exists(&conn, "dispatcher_messages", "tool_artifacts_json", "TEXT")?;
        Ok(())
    }

    fn connect(&self) -> Result<Connection> {
        Connection::open(&self.path).with_context(|| format!("open {}", self.path.display()))
    }

    fn find_dialogue_cutoff_rowid(
        &self,
        conn: &Connection,
        workspace_id: &str,
        max_dialogues: usize,
    ) -> Result<i64> {
        let mut stmt = conn.prepare(
            "SELECT rowid
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND role = 'user'
             ORDER BY rowid DESC
             LIMIT ?2",
        )?;
        let rowids = stmt
            .query_map(params![workspace_id, max_dialogues as i64], |row| {
                row.get::<_, i64>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("load dispatcher dialogue boundaries")?;

        Ok(rowids.into_iter().min().unwrap_or(0))
    }
}

impl DispatcherSessionTokenUsageSource {
    fn as_sql_value(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Summary => "summary",
        }
    }

    fn from_sql_value(value: String) -> Self {
        match value.as_str() {
            "summary" => Self::Summary,
            _ => Self::Primary,
        }
    }
}

fn ensure_column_exists(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    match conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
        [],
    ) {
        Ok(_) => Ok(()),
        Err(rusqlite::Error::SqliteFailure(_, Some(message)))
            if message.contains("duplicate column name") =>
        {
            Ok(())
        }
        Err(error) => {
            Err(error).with_context(|| format!("ensure column {column} exists on table {table}"))
        }
    }
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn default_context_window_capacity(_model: &str) -> u64 {
    DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS
}

fn normalize_summary_model(summary_model: &str) -> String {
    let trimmed = summary_model.trim();
    if trimmed.is_empty() {
        DEFAULT_SUMMARY_MODEL.to_string()
    } else {
        trimmed.to_string()
    }
}

fn latest_user_message_rowid(conn: &Connection, workspace_id: &str) -> Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(rowid), 0)
         FROM dispatcher_messages
         WHERE workspace_id = ?1 AND role = 'user'",
        params![workspace_id],
        |row| row.get(0),
    )
    .context("load latest dispatcher user message rowid")
}

fn map_chat_message_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatMessage> {
    let tool_calls_json: Option<String> = row.get(4)?;
    let tool_calls = tool_calls_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<Vec<OutboundToolCall>>(json).ok());

    Ok(ChatMessage {
        role: row.get(0)?,
        content: row.get(1)?,
        tool_call_id: row.get(2)?,
        name: row.get(3)?,
        tool_calls,
    })
}

fn parse_tool_artifact_refs(raw: Option<String>) -> Vec<DispatcherToolArtifactRef> {
    raw.as_deref()
        .and_then(|json| serde_json::from_str::<Vec<DispatcherToolArtifactRef>>(json).ok())
        .unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::{DispatcherDb, ToolArtifactDraft};

    fn create_test_db() -> (DispatcherDb, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("jkcodingagent-db-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("create temp db root");
        let db = DispatcherDb::new(root.join("dispatcher.sqlite3")).expect("create dispatcher db");
        (db, root)
    }

    fn cleanup_test_db(root: PathBuf) {
        let _ = fs::remove_dir_all(root);
    }

    fn sample_artifact() -> ToolArtifactDraft {
        ToolArtifactDraft {
            kind: "tool_raw_output".to_string(),
            title: "exec 原始结果".to_string(),
            preview: "line 1 / line 2".to_string(),
            content: "line 1\nline 2".to_string(),
            char_count: 13,
            line_count: 2,
        }
    }

    #[test]
    fn update_session_title_persists_latest_title() {
        let (db, root) = create_test_db();
        let session = db.create_session("project-1", "新会话").unwrap();

        let updated = db
            .update_session_title(&session.id, "修复会话命名")
            .unwrap()
            .expect("session should exist");

        assert_eq!(updated.title, "修复会话命名");
        assert_eq!(updated.project_id, "project-1");
        assert!(updated.updated_at >= session.updated_at);
        assert_eq!(
            db.list_sessions("project-1").unwrap()[0].title,
            "修复会话命名"
        );

        cleanup_test_db(root);
    }

    #[test]
    fn tool_result_separates_display_content_from_context_payload() {
        let (db, root) = create_test_db();
        let session = db.create_session("project-1", "测试会话").unwrap();
        db.add_visible_message(&session.id, "user", "检查工具结果")
            .unwrap();
        let message = db
            .add_visible_tool_result(
                &session.id,
                "展示摘要",
                "上下文压缩",
                Some("call-1"),
                Some("exec"),
                Some("conservative_summary"),
                &[sample_artifact()],
            )
            .unwrap();

        assert_eq!(message.content, "展示摘要");
        assert_eq!(message.tool_artifacts.len(), 1);

        let visible_messages = db.list_visible_messages(&session.id).unwrap();
        assert_eq!(visible_messages.last().unwrap().content, "展示摘要");
        assert_eq!(visible_messages.last().unwrap().tool_artifacts.len(), 1);

        let llm_history = db.load_llm_history(&session.id).unwrap();
        let tool_message = llm_history
            .iter()
            .find(|message| message.role == "tool")
            .expect("tool message should exist in llm history");
        assert_eq!(tool_message.content, "上下文压缩");

        let artifact = db
            .get_tool_artifact(&session.id, &message.tool_artifacts[0].id)
            .unwrap();
        assert_eq!(artifact.content, "line 1\nline 2");

        cleanup_test_db(root);
    }

    #[test]
    fn clear_messages_removes_tool_artifacts_for_session() {
        let (db, root) = create_test_db();
        let session = db.create_session("project-1", "测试会话").unwrap();
        let message = db
            .add_visible_tool_result(
                &session.id,
                "展示摘要",
                "上下文压缩",
                Some("call-1"),
                Some("exec"),
                Some("conservative_summary"),
                &[sample_artifact()],
            )
            .unwrap();

        db.clear_messages(&session.id).unwrap();

        assert!(db.list_visible_messages(&session.id).unwrap().is_empty());
        assert!(db
            .get_tool_artifact(&session.id, &message.tool_artifacts[0].id)
            .is_err());

        cleanup_test_db(root);
    }

    #[test]
    fn delete_session_removes_tool_artifacts_with_session() {
        let (db, root) = create_test_db();
        let session = db.create_session("project-1", "测试会话").unwrap();
        let message = db
            .add_visible_tool_result(
                &session.id,
                "展示摘要",
                "上下文压缩",
                Some("call-1"),
                Some("exec"),
                Some("conservative_summary"),
                &[sample_artifact()],
            )
            .unwrap();

        db.delete_session(&session.id).unwrap();

        assert!(db.list_sessions("project-1").unwrap().is_empty());
        assert!(db
            .get_tool_artifact(&session.id, &message.tool_artifacts[0].id)
            .is_err());

        cleanup_test_db(root);
    }
}

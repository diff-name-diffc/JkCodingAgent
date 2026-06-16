//! 会话 token 用量（dispatcher_session_token_usage 表）的读写与异步包装。

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::agent::llm::LlmUsage;

use super::util::{default_context_window_capacity, now, usage_total_tokens};
use super::DispatcherDb;

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

impl DispatcherDb {
    pub fn upsert_session_token_usage(
        &self,
        workspace_id: &str,
        model: &str,
        source_kind: DispatcherSessionTokenUsageSource,
        usage: &LlmUsage,
    ) -> Result<DispatcherSessionTokenUsageRecord> {
        let updated_at = now();
        let record = DispatcherSessionTokenUsageRecord {
            workspace_id: workspace_id.to_string(),
            model: model.to_string(),
            source_kind,
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage_total_tokens(usage),
            cached_tokens: usage.cached_tokens(),
            context_window_tokens: usage.prompt_tokens,
            context_window_capacity: default_context_window_capacity(model),
            updated_at,
        };
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO dispatcher_session_token_usage (
                workspace_id, model, source_kind, prompt_tokens, completion_tokens, total_tokens,
                cached_tokens, context_window_tokens, context_window_capacity, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(workspace_id, model, source_kind) DO UPDATE SET
                prompt_tokens = dispatcher_session_token_usage.prompt_tokens + excluded.prompt_tokens,
                completion_tokens = dispatcher_session_token_usage.completion_tokens + excluded.completion_tokens,
                total_tokens = dispatcher_session_token_usage.total_tokens + excluded.total_tokens,
                cached_tokens = dispatcher_session_token_usage.cached_tokens + excluded.cached_tokens,
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
        self.get_session_token_usage_record(&conn, workspace_id, model, source_kind)
    }

    fn get_session_token_usage_record(
        &self,
        conn: &Connection,
        workspace_id: &str,
        model: &str,
        source_kind: DispatcherSessionTokenUsageSource,
    ) -> Result<DispatcherSessionTokenUsageRecord> {
        conn.query_row(
            "SELECT workspace_id, model, source_kind, prompt_tokens, completion_tokens, total_tokens,
                    cached_tokens, context_window_tokens, context_window_capacity, updated_at
             FROM dispatcher_session_token_usage
             WHERE workspace_id = ?1 AND model = ?2 AND source_kind = ?3",
            params![workspace_id, model, source_kind.as_sql_value()],
            |row| {
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
            },
        )
        .context("load dispatcher session token usage after upsert")
    }

    pub fn list_session_token_usage(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<DispatcherSessionTokenUsageRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT workspace_id, model, source_kind, prompt_tokens, completion_tokens, total_tokens,
                    cached_tokens, context_window_tokens, context_window_capacity, updated_at
             FROM dispatcher_session_token_usage
             WHERE workspace_id = ?1
             ORDER BY updated_at DESC, model ASC, source_kind ASC",
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
    pub async fn upsert_session_token_usage_async(
        &self,
        workspace_id: &str,
        model: &str,
        source_kind: DispatcherSessionTokenUsageSource,
        usage: &LlmUsage,
    ) -> Result<DispatcherSessionTokenUsageRecord> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        let model = model.to_string();
        let usage = usage.clone();
        tokio::task::spawn_blocking(move || {
            db.upsert_session_token_usage(&wid, &model, source_kind, &usage)
        })
        .await
        .context("upsert_session_token_usage spawn_blocking")?
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

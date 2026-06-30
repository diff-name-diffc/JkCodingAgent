//! 工具运行台账：记录每次 agent tool call 的生命周期、结果状态和观测元数据。

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::util::now;
use super::DispatcherDb;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherToolRunRecord {
    pub id: String,
    pub workspace_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub provider: String,
    pub category: String,
    pub status: String,
    pub arguments_json: String,
    pub effective_arguments_json: String,
    pub result_mode: Option<String>,
    pub message_id: Option<String>,
    pub error_kind: Option<String>,
    pub error_message: Option<String>,
    pub action_kind: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub duration_ms: u64,
    pub metadata_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct NewToolRun {
    pub workspace_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub provider: String,
    pub category: String,
    pub arguments_json: String,
    pub effective_arguments_json: String,
    pub metadata_json: String,
}

#[derive(Debug, Clone, Default)]
pub struct FinishToolRun {
    pub status: String,
    pub result_mode: Option<String>,
    pub message_id: Option<String>,
    pub error_kind: Option<String>,
    pub error_message: Option<String>,
    pub action_kind: Option<String>,
    pub metadata_json: Option<String>,
}

impl DispatcherDb {
    pub fn create_tool_run(&self, run: NewToolRun) -> Result<DispatcherToolRunRecord> {
        let conn = self.conn()?;
        let id = Uuid::new_v4().to_string();
        let timestamp = now();
        conn.execute(
            "INSERT INTO dispatcher_tool_runs (
                id, workspace_id, tool_call_id, tool_name, provider, category, status,
                arguments_json, effective_arguments_json, metadata_json, created_at, updated_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'planned', ?7, ?8, ?9, ?10, ?10)",
            params![
                &id,
                &run.workspace_id,
                &run.tool_call_id,
                &run.tool_name,
                &run.provider,
                &run.category,
                &run.arguments_json,
                &run.effective_arguments_json,
                &run.metadata_json,
                &timestamp
            ],
        )
        .context("create dispatcher tool run")?;
        self.load_tool_run(&id)
    }

    pub fn mark_tool_run_started(&self, id: &str) -> Result<DispatcherToolRunRecord> {
        let conn = self.conn()?;
        let timestamp = now();
        conn.execute(
            "UPDATE dispatcher_tool_runs
             SET status = 'running', started_at = COALESCE(started_at, ?1), updated_at = ?1
             WHERE id = ?2",
            params![&timestamp, id],
        )
        .context("mark dispatcher tool run started")?;
        self.load_tool_run(id)
    }

    pub fn finish_tool_run(
        &self,
        id: &str,
        finish: FinishToolRun,
    ) -> Result<DispatcherToolRunRecord> {
        let conn = self.conn()?;
        let timestamp = now();
        conn.execute(
            "UPDATE dispatcher_tool_runs
             SET status = ?1,
                 result_mode = ?2,
                 message_id = COALESCE(?3, message_id),
                 error_kind = ?4,
                 error_message = ?5,
                 action_kind = ?6,
                 finished_at = ?7,
                 duration_ms = CASE
                     WHEN started_at IS NULL THEN 0
                     ELSE CAST((julianday(?7) - julianday(started_at)) * 86400000 AS INTEGER)
                 END,
                 metadata_json = COALESCE(?8, metadata_json),
                 updated_at = ?7
             WHERE id = ?9",
            params![
                &finish.status,
                &finish.result_mode,
                &finish.message_id,
                &finish.error_kind,
                &finish.error_message,
                &finish.action_kind,
                &timestamp,
                &finish.metadata_json,
                id
            ],
        )
        .context("finish dispatcher tool run")?;
        self.load_tool_run(id)
    }

    #[allow(dead_code)]
    pub fn attach_tool_run_message(
        &self,
        id: &str,
        message_id: &str,
    ) -> Result<DispatcherToolRunRecord> {
        let conn = self.conn()?;
        let timestamp = now();
        conn.execute(
            "UPDATE dispatcher_tool_runs
             SET message_id = ?1, updated_at = ?2
             WHERE id = ?3",
            params![message_id, &timestamp, id],
        )
        .context("attach dispatcher tool run message")?;
        self.load_tool_run(id)
    }

    pub fn load_tool_run(&self, id: &str) -> Result<DispatcherToolRunRecord> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, workspace_id, tool_call_id, tool_name, provider, category, status,
                    arguments_json, effective_arguments_json, result_mode, message_id,
                    error_kind, error_message, action_kind, started_at, finished_at,
                    duration_ms, metadata_json, created_at, updated_at
             FROM dispatcher_tool_runs
             WHERE id = ?1",
            params![id],
            map_tool_run,
        )
        .optional()
        .context("load dispatcher tool run")?
        .with_context(|| format!("dispatcher tool run not found: {id}"))
    }

    #[allow(dead_code)]
    pub fn list_recent_tool_runs(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<DispatcherToolRunRecord>> {
        let conn = self.conn()?;
        let limit = limit.clamp(1, 200) as i64;
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, tool_call_id, tool_name, provider, category, status,
                    arguments_json, effective_arguments_json, result_mode, message_id,
                    error_kind, error_message, action_kind, started_at, finished_at,
                    duration_ms, metadata_json, created_at, updated_at
             FROM dispatcher_tool_runs
             WHERE workspace_id = ?1
             ORDER BY created_at DESC, rowid DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![workspace_id, limit], map_tool_run)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("list recent dispatcher tool runs")
    }

    pub async fn create_tool_run_async(&self, run: NewToolRun) -> Result<DispatcherToolRunRecord> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || db.create_tool_run(run))
            .await
            .context("create_tool_run spawn_blocking")?
    }

    pub async fn mark_tool_run_started_async(&self, id: &str) -> Result<DispatcherToolRunRecord> {
        let db = self.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || db.mark_tool_run_started(&id))
            .await
            .context("mark_tool_run_started spawn_blocking")?
    }

    pub async fn finish_tool_run_async(
        &self,
        id: &str,
        finish: FinishToolRun,
    ) -> Result<DispatcherToolRunRecord> {
        let db = self.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || db.finish_tool_run(&id, finish))
            .await
            .context("finish_tool_run spawn_blocking")?
    }

    #[allow(dead_code)]
    pub async fn load_tool_run_async(&self, id: &str) -> Result<DispatcherToolRunRecord> {
        let db = self.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || db.load_tool_run(&id))
            .await
            .context("load_tool_run spawn_blocking")?
    }

    #[allow(dead_code)]
    pub async fn list_recent_tool_runs_async(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<DispatcherToolRunRecord>> {
        let db = self.clone();
        let workspace_id = workspace_id.to_string();
        tokio::task::spawn_blocking(move || db.list_recent_tool_runs(&workspace_id, limit))
            .await
            .context("list_recent_tool_runs spawn_blocking")?
    }
}

fn map_tool_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<DispatcherToolRunRecord> {
    let duration_ms = row.get::<_, i64>(16).map(|value| value.max(0) as u64)?;
    Ok(DispatcherToolRunRecord {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        tool_call_id: row.get(2)?,
        tool_name: row.get(3)?,
        provider: row.get(4)?,
        category: row.get(5)?,
        status: row.get(6)?,
        arguments_json: row.get(7)?,
        effective_arguments_json: row.get(8)?,
        result_mode: row.get(9)?,
        message_id: row.get(10)?,
        error_kind: row.get(11)?,
        error_message: row.get(12)?,
        action_kind: row.get(13)?,
        started_at: row.get(14)?,
        finished_at: row.get(15)?,
        duration_ms,
        metadata_json: row.get(17)?,
        created_at: row.get(18)?,
        updated_at: row.get(19)?,
    })
}

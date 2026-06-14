//! Python 代码块执行记录（python_code_runs 表）的读写。

use anyhow::{Context, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::DispatcherDb;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PythonCodeRunRecord {
    pub run_id: String,
    pub workspace_id: String,
    pub message_id: String,
    pub code_block_index: u32,
    pub code_hash: String,
    pub code: String,
    pub status: String,
    pub stdout: String,
    pub stderr: String,
    pub installed_packages_json: String,
    pub tool_events_json: String,
    pub explanation_markdown: String,
    pub error_reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

fn map_python_code_run_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<PythonCodeRunRecord> {
    Ok(PythonCodeRunRecord {
        run_id: row.get(0)?,
        workspace_id: row.get(1)?,
        message_id: row.get(2)?,
        code_block_index: row.get::<_, i64>(3)? as u32,
        code_hash: row.get(4)?,
        code: row.get(5)?,
        status: row.get(6)?,
        stdout: row.get(7)?,
        stderr: row.get(8)?,
        installed_packages_json: row.get(9)?,
        tool_events_json: row.get(10)?,
        explanation_markdown: row.get(11)?,
        error_reason: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

impl DispatcherDb {
    pub fn list_python_code_runs(
        &self,
        workspace_id: &str,
        message_id: Option<&str>,
    ) -> Result<Vec<PythonCodeRunRecord>> {
        let conn = self.conn()?;
        let sql = if message_id.is_some() {
            "SELECT run_id, workspace_id, message_id, code_block_index, code_hash, code, status,
                    stdout, stderr, installed_packages_json, tool_events_json, explanation_markdown,
                    error_reason, created_at, updated_at
             FROM python_code_runs
             WHERE workspace_id = ?1 AND message_id = ?2
             ORDER BY code_block_index ASC"
        } else {
            "SELECT run_id, workspace_id, message_id, code_block_index, code_hash, code, status,
                    stdout, stderr, installed_packages_json, tool_events_json, explanation_markdown,
                    error_reason, created_at, updated_at
             FROM python_code_runs
             WHERE workspace_id = ?1
             ORDER BY updated_at DESC, message_id ASC, code_block_index ASC"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = if let Some(message_id) = message_id {
            stmt.query_map(
                params![workspace_id, message_id],
                map_python_code_run_record,
            )?
        } else {
            stmt.query_map(params![workspace_id], map_python_code_run_record)?
        };

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("list python code runs")
    }

    pub fn upsert_python_code_run(&self, record: &PythonCodeRunRecord) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO python_code_runs (
                run_id, workspace_id, message_id, code_block_index, code_hash, code, status,
                stdout, stderr, installed_packages_json, tool_events_json, explanation_markdown,
                error_reason, created_at, updated_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(workspace_id, message_id, code_block_index) DO UPDATE SET
                run_id = excluded.run_id,
                code_hash = excluded.code_hash,
                code = excluded.code,
                status = excluded.status,
                stdout = excluded.stdout,
                stderr = excluded.stderr,
                installed_packages_json = excluded.installed_packages_json,
                tool_events_json = excluded.tool_events_json,
                explanation_markdown = excluded.explanation_markdown,
                error_reason = excluded.error_reason,
                updated_at = excluded.updated_at",
            params![
                &record.run_id,
                &record.workspace_id,
                &record.message_id,
                record.code_block_index as i64,
                &record.code_hash,
                &record.code,
                &record.status,
                &record.stdout,
                &record.stderr,
                &record.installed_packages_json,
                &record.tool_events_json,
                &record.explanation_markdown,
                &record.error_reason,
                &record.created_at,
                &record.updated_at,
            ],
        )
        .context("upsert python code run")?;
        Ok(())
    }

    pub fn clear_python_code_run(
        &self,
        workspace_id: &str,
        message_id: &str,
        code_block_index: u32,
    ) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM python_code_runs
             WHERE workspace_id = ?1 AND message_id = ?2 AND code_block_index = ?3",
            params![workspace_id, message_id, code_block_index as i64],
        )
        .context("clear python code run")?;
        Ok(())
    }
}

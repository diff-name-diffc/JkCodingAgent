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
        // DB i64 → u32 不再用 as 强转，越界显式报错而非静默回绕。
        code_block_index: u32_from_sql(row.get::<_, i64>(3)?, 3)?,
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

fn u32_from_sql(value: i64, column: usize) -> rusqlite::Result<u32> {
    u32::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Integer, Box::new(error))
    })
}

impl DispatcherDb {
    pub fn list_python_code_runs(
        &self,
        workspace_id: &str,
        message_id: Option<&str>,
    ) -> Result<Vec<PythonCodeRunRecord>> {
        let conn = self.conn()?;
        // updated_at 是 RFC3339 文本且小数精度可变，直接按字符串排序在混合精度/
        // 混合时区偏移时可能错位；改用 julianday 数值解释排序，异常值解析为 NULL
        // 时自然排到最后，不影响其余行的正确顺序。
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
             ORDER BY julianday(updated_at) DESC, message_id ASC, code_block_index ASC"
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
                i64::from(record.code_block_index),
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
            params![
                workspace_id,
                message_id,
                i64::from(code_block_index)
            ],
        )
        .context("clear python code run")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> DispatcherDb {
        let path = std::env::temp_dir().join(format!(
            "jkcodingagent-python-runs-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));
        DispatcherDb::new(path).expect("create test dispatcher db")
    }

    /// 建一个会话和 n 条消息，返回 (session_id, message_ids)，
    /// 满足 python_code_runs.message_id → dispatcher_messages(id) 外键。
    fn setup_session_with_messages(db: &DispatcherDb, count: usize) -> (String, Vec<String>) {
        let session = db
            .create_chat_session("python runs", Some("tech"))
            .expect("create chat session");
        let mut message_ids = Vec::new();
        for index in 0..count {
            let segments_json =
                super::super::content::content_to_segments_json(&format!("block {index}"));
            let message = db
                .add_visible_message_from_segments(&session.id, "user", segments_json)
                .expect("add message");
            message_ids.push(message.id);
        }
        (session.id, message_ids)
    }

    fn record(
        workspace_id: &str,
        message_id: &str,
        index: u32,
        updated_at: &str,
    ) -> PythonCodeRunRecord {
        PythonCodeRunRecord {
            run_id: uuid::Uuid::new_v4().to_string(),
            workspace_id: workspace_id.to_string(),
            message_id: message_id.to_string(),
            code_block_index: index,
            code_hash: format!("hash-{index}"),
            code: format!("print({index})"),
            status: "succeeded".to_string(),
            stdout: String::new(),
            stderr: String::new(),
            installed_packages_json: "[]".to_string(),
            tool_events_json: "[]".to_string(),
            explanation_markdown: String::new(),
            error_reason: None,
            created_at: updated_at.to_string(),
            updated_at: updated_at.to_string(),
        }
    }

    #[test]
    fn listing_orders_by_updated_at_desc_numerically() {
        let db = test_db();
        let (session_id, message_ids) = setup_session_with_messages(&db, 3);
        // 混合小数精度的 RFC3339 时间戳：按数值解释排序，最新在前。
        let base = "2026-08-11T10:00:00";
        db.upsert_python_code_run(&record(&session_id, &message_ids[0], 0, &format!("{base}.1+00:00")))
            .expect("insert run 1");
        db.upsert_python_code_run(&record(&session_id, &message_ids[1], 0, &format!("{base}.25+00:00")))
            .expect("insert run 2");
        db.upsert_python_code_run(&record(&session_id, &message_ids[2], 0, &format!("{base}+00:00")))
            .expect("insert run 3");

        let runs = db
            .list_python_code_runs(&session_id, None)
            .expect("list python runs");
        let message_order: Vec<&str> = runs
            .iter()
            .map(|run| run.message_id.as_str())
            .collect();
        assert_eq!(
            message_order,
            vec![message_ids[1].as_str(), message_ids[0].as_str(), message_ids[2].as_str()],
            "应按 updated_at 数值从新到旧排序"
        );
    }

    #[test]
    fn listing_scoped_by_message_orders_by_block_index() {
        let db = test_db();
        let (session_id, message_ids) = setup_session_with_messages(&db, 1);
        let message_id = &message_ids[0];
        let ts = "2026-08-11T10:00:00+00:00";
        db.upsert_python_code_run(&record(&session_id, message_id, 2, ts))
            .expect("insert block 2");
        db.upsert_python_code_run(&record(&session_id, message_id, 0, ts))
            .expect("insert block 0");
        db.upsert_python_code_run(&record(&session_id, message_id, 1, ts))
            .expect("insert block 1");

        let runs = db
            .list_python_code_runs(&session_id, Some(message_id))
            .expect("list python runs for message");
        let indices: Vec<u32> = runs.iter().map(|run| run.code_block_index).collect();
        assert_eq!(indices, vec![0, 1, 2]);
    }
}

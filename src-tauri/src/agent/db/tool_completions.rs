//! 工具终态、完整产物与完成事件同事务提交；协调器独占消息交付。
mod delivery;
mod recovery;

use anyhow::{ensure, Context, Result};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::Serialize;

use super::{util::now, DispatcherDb, ToolArtifactDraft};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolCompletion {
    pub event_id: i64,
    pub tool_run_id: String,
    pub tool_name: String,
    pub tool_call_id: String,
    pub dispatch_round: i64,
    pub agent_run_id: String,
    pub scope_id: String,
    pub status: String,
    pub error_kind: Option<String>,
    pub fatal: bool,
    pub retryable: bool,
    pub display_content: String,
    pub context_payload: String,
    pub result_mode: String,
    pub usage_json: Option<String>,
    pub delivery_message_id: Option<String>,
    pub observed_request_step: Option<i64>,
}

pub(crate) struct CompletionDraft {
    pub tool_run_id: String,
    pub status: String,
    pub error_kind: Option<String>,
    pub fatal: bool,
    pub retryable: bool,
    pub display_content: String,
    pub context_payload: String,
    pub result_mode: String,
    pub usage_json: Option<String>,
    pub artifact: ToolArtifactDraft,
}

impl DispatcherDb {
    /// 重复结算返回首个事件，不重复写产物或用量；事务提交前不得发布通知。
    pub(crate) fn settle_tool_completion(&self, draft: CompletionDraft) -> Result<i64> {
        ensure!(
            matches!(
                draft.status.as_str(),
                "succeeded"
                    | "recoverable_error"
                    | "fatal_error"
                    | "cancelled"
                    | "failed"
                    | "internal_error"
            ),
            "非法工具终态：{}",
            draft.status
        );
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(event_id) = tx
            .query_row(
                "SELECT event_id FROM dispatcher_tool_completions WHERE tool_run_id = ?1",
                [&draft.tool_run_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        {
            tx.commit()?;
            return Ok(event_id);
        }
        let (workspace, call_id, tool_name, run, scope, status): (
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
        ) = tx
            .query_row(
                "SELECT workspace_id, tool_call_id, tool_name, agent_run_id, scope_id, status
             FROM dispatcher_tool_runs WHERE id = ?1",
                [&draft.tool_run_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .context("读取待结算工具台账")?;
        ensure!(
            matches!(status.as_str(), "planned" | "running"),
            "工具已终结但没有完成事件：{}",
            draft.tool_run_id
        );
        let run = run.context("工具缺少 agent_run_id")?;
        let scope = scope.context("工具缺少 scope_id")?;
        let timestamp = now();
        let artifact = draft.artifact;
        tx.execute(
            "INSERT INTO dispatcher_tool_artifacts
             (id, workspace_id, tool_call_id, tool_run_id, tool_name, title, kind,
              preview, content, char_count, line_count, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                uuid::Uuid::new_v4().to_string(),
                workspace,
                call_id,
                draft.tool_run_id,
                tool_name,
                artifact.title,
                artifact.kind,
                artifact.preview,
                artifact.content,
                i64::try_from(artifact.char_count)?,
                i64::try_from(artifact.line_count)?,
                timestamp
            ],
        )?;
        tx.execute(
            "UPDATE dispatcher_tool_runs SET status = ?1, phase = NULL, result_mode = ?2,
             error_kind = ?3, finished_at = ?4, updated_at = ?4,
             duration_ms = CASE WHEN started_at IS NULL THEN 0 ELSE
                MAX(0, CAST((julianday(?4) - julianday(started_at)) * 86400000 AS INTEGER)) END
             WHERE id = ?5",
            params![
                draft.status,
                draft.result_mode,
                draft.error_kind,
                timestamp,
                draft.tool_run_id
            ],
        )?;
        tx.execute(
            "INSERT INTO dispatcher_tool_completions
             (tool_run_id, agent_run_id, scope_id, status, error_kind, fatal, retryable,
              display_content, context_payload, result_mode, usage_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                draft.tool_run_id,
                run,
                scope,
                draft.status,
                draft.error_kind,
                draft.fatal,
                draft.retryable,
                draft.display_content,
                draft.context_payload,
                draft.result_mode,
                draft.usage_json,
                timestamp
            ],
        )?;
        let event_id = tx.last_insert_rowid();
        tx.commit().context("原子提交工具终态、产物及完成事件")?;
        Ok(event_id)
    }

    /// 快照上界由协调器在请求开始时捕获；请求期间的新事件留到下轮。
    pub(crate) fn pending_tool_completions(
        &self,
        agent_run_id: &str,
        scope_id: &str,
        through_event_id: i64,
    ) -> Result<Vec<ToolCompletion>> {
        let conn = self.conn()?;
        let mut query = conn.prepare(
            "SELECT c.*, r.tool_name, r.tool_call_id, r.dispatch_round FROM dispatcher_tool_completions c
             JOIN dispatcher_tool_runs r ON r.id = c.tool_run_id
             WHERE c.agent_run_id = ?1 AND c.scope_id = ?2 AND event_id <= ?3
             AND observed_request_step IS NULL ORDER BY event_id",
        )?;
        let rows = query.query_map(
            params![agent_run_id, scope_id, through_event_id],
            map_completion,
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests;

fn map_completion(row: &rusqlite::Row<'_>) -> rusqlite::Result<ToolCompletion> {
    Ok(ToolCompletion {
        event_id: row.get("event_id")?,
        tool_run_id: row.get("tool_run_id")?,
        tool_name: row.get("tool_name")?,
        tool_call_id: row.get("tool_call_id")?,
        dispatch_round: row.get::<_, Option<i64>>("dispatch_round")?.unwrap_or(0),
        agent_run_id: row.get("agent_run_id")?,
        scope_id: row.get("scope_id")?,
        status: row.get("status")?,
        error_kind: row.get("error_kind")?,
        fatal: row.get("fatal")?,
        retryable: row.get("retryable")?,
        display_content: row.get("display_content")?,
        context_payload: row.get("context_payload")?,
        result_mode: row.get("result_mode")?,
        usage_json: row.get("usage_json")?,
        delivery_message_id: row.get("delivery_message_id")?,
        observed_request_step: row.get("observed_request_step")?,
    })
}

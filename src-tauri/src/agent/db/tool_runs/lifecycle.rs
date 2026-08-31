//! 单条工具运行的生命周期状态机：planned → running → 终态。
//!
//! 状态单向推进：终态只能由第一个 finish 落定，重复/乱序调用幂等返回当前快照；
//! 可选字段统一 COALESCE 保留语义，不被后续 finish 清空。

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use uuid::Uuid;

use super::{
    load_tool_run_on_conn, DispatcherToolRunRecord, FinishToolRun, NewToolRun, ToolRunTraceContext,
};
use crate::agent::db::util::now;
use crate::agent::db::DispatcherDb;

impl DispatcherDb {
    #[cfg(test)]
    pub fn create_tool_run(&self, run: NewToolRun) -> Result<DispatcherToolRunRecord> {
        self.create_tool_run_with_trace(run, ToolRunTraceContext::default())
    }

    pub fn create_tool_run_with_trace(
        &self,
        run: NewToolRun,
        trace: ToolRunTraceContext,
    ) -> Result<DispatcherToolRunRecord> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin create dispatcher tool run transaction")?;
        let id = Uuid::new_v4().to_string();
        let timestamp = now();
        let origin = trace.origin.trim();
        if origin.is_empty() {
            anyhow::bail!("dispatcher tool run origin must not be empty");
        }
        let sequence = i64::try_from(trace.sequence)
            .context("dispatcher tool run sequence exceeds sqlite INTEGER range")?;

        if let Some(parent_run_id) = trace.parent_run_id.as_deref() {
            let parent_workspace_id = tx
                .query_row(
                    "SELECT workspace_id FROM dispatcher_tool_runs WHERE id = ?1",
                    params![parent_run_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .context("load parent dispatcher tool run")?
                .with_context(|| {
                    format!("parent dispatcher tool run not found: {parent_run_id}")
                })?;
            if parent_workspace_id != run.workspace_id {
                anyhow::bail!(
                    "parent dispatcher tool run {parent_run_id} belongs to workspace {parent_workspace_id}, not {}",
                    run.workspace_id
                );
            }
        }

        tx.execute(
            "INSERT INTO dispatcher_tool_runs (
                id, workspace_id, tool_call_id, parent_run_id, origin, step_id, sequence,
                tool_name, provider, category, status, arguments_json,
                effective_arguments_json, metadata_json, created_at, updated_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'planned', ?11, ?12, ?13, ?14, ?14)",
            params![
                &id,
                &run.workspace_id,
                &run.tool_call_id,
                &trace.parent_run_id,
                origin,
                &trace.step_id,
                sequence,
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
        tx.commit()
            .context("commit create dispatcher tool run transaction")?;
        load_tool_run_on_conn(&conn, &id)
    }

    pub fn mark_tool_run_started(&self, id: &str) -> Result<DispatcherToolRunRecord> {
        let conn = self.conn()?;
        let timestamp = now();
        // 生命周期单向推进：仅允许 planned → running，
        // 已在运行或已到终态的记录不得被回退。
        conn.execute(
            "UPDATE dispatcher_tool_runs
             SET status = 'running', started_at = COALESCE(started_at, ?1), updated_at = ?1
             WHERE id = ?2 AND status = 'planned'",
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
        let mut conn = self.conn()?;
        // IMMEDIATE：事务内先读状态再写入，避免延迟事务升级写锁时的 SQLITE_BUSY。
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin finish dispatcher tool run transaction")?;
        let current: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT status, started_at FROM dispatcher_tool_runs WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .context("load dispatcher tool run state before finish")?;
        let Some((status, started_at)) = current else {
            anyhow::bail!("dispatcher tool run not found: {id}");
        };
        if is_terminal_run_status(&status) {
            // 状态守卫：终态只能由第一个 finish 落定，重复/乱序 finish 不得覆盖，
            // 幂等返回当前快照。
            tx.commit()
                .context("commit finish dispatcher tool run no-op")?;
            return load_tool_run_on_conn(&conn, id);
        }

        let finished_at = now();
        // 时长在 Rust 侧计算（不再依赖 SQL julianday 解析文本时间戳）；
        // started_at 缺失或无法解析时容错为 0。
        let duration_ms = duration_since_started_ms(started_at.as_deref(), &finished_at);
        // 可选字段统一 COALESCE 保留语义：未提供新值时保留既有值，
        // 避免重复/乱序 finish 清空已落定的字段。
        let changed = tx
            .execute(
                "UPDATE dispatcher_tool_runs
                 SET status = ?1,
                     result_mode = COALESCE(?2, result_mode),
                     message_id = COALESCE(?3, message_id),
                     error_kind = COALESCE(?4, error_kind),
                     error_message = COALESCE(?5, error_message),
                     action_kind = COALESCE(?6, action_kind),
                     finished_at = ?7,
                     duration_ms = ?8,
                     metadata_json = COALESCE(?9, metadata_json),
                     updated_at = ?7
                 WHERE id = ?10
                   AND status NOT IN (
                       'succeeded', 'recoverable_error', 'fatal_error', 'cancelled',
                       'failed', 'internal_error'
                   )",
                params![
                    &finish.status,
                    &finish.result_mode,
                    &finish.message_id,
                    &finish.error_kind,
                    &finish.error_message,
                    &finish.action_kind,
                    &finished_at,
                    duration_ms,
                    &finish.metadata_json,
                    id
                ],
            )
            .context("finish dispatcher tool run")?;
        if changed == 0 {
            // 与状态守卫的双重保险：其他写者抢先推进到终态时，退化为只读返回。
            tx.commit()
                .context("commit finish dispatcher tool run no-op")?;
            return load_tool_run_on_conn(&conn, id);
        }
        tx.commit().context("commit finish dispatcher tool run")?;
        load_tool_run_on_conn(&conn, id)
    }

    pub fn load_tool_run(&self, id: &str) -> Result<DispatcherToolRunRecord> {
        let conn = self.conn()?;
        load_tool_run_on_conn(&conn, id)
    }
}

/// 运行到达这些状态后生命周期即结束，不得被后续 finish 覆盖（单向推进）。
fn is_terminal_run_status(status: &str) -> bool {
    matches!(
        status,
        "succeeded"
            | "recoverable_error"
            | "fatal_error"
            | "cancelled"
            | "failed"
            | "internal_error"
    )
}

/// 在 Rust 侧由 RFC3339 文本时间戳计算时长（毫秒）。
/// started_at 缺失、任一时间戳无法解析时容错为 0，不产生 NULL。
fn duration_since_started_ms(started_at: Option<&str>, finished_at: &str) -> i64 {
    let Some(started_at) = started_at else {
        return 0;
    };
    let (Ok(started), Ok(finished)) = (
        chrono::DateTime::parse_from_rfc3339(started_at),
        chrono::DateTime::parse_from_rfc3339(finished_at),
    ) else {
        return 0;
    };
    (finished - started).num_milliseconds().max(0)
}

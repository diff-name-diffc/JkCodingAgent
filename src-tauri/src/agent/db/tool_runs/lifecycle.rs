//! 单条工具运行的生命周期状态机：planned → running → 终态。
//!
//! 终态唯一由 `tool_completions::settle_tool_completion` 落定（首个完成事件
//! wins，重复结算幂等返回同一事件）；本模块只保留登记（测试种子路径）与
//! running 推进。历史上的独立 `finish_tool_run` 裸路径已随 loop 侧三段式
//! 台账一并删除（P0-3 单写路径收敛）。

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

#[cfg(test)]
use rusqlite::TransactionBehavior;

use super::{load_tool_run_on_conn, DispatcherToolRunRecord, NewToolRun, ToolRunTraceContext};
use crate::agent::db::util::now;
use crate::agent::db::DispatcherDb;

impl DispatcherDb {
    #[cfg(test)]
    pub fn create_tool_run(&self, run: NewToolRun) -> Result<DispatcherToolRunRecord> {
        self.create_tool_run_with_trace(run, ToolRunTraceContext::default())
    }

    #[cfg(test)]
    pub fn create_tool_run_with_trace(
        &self,
        run: NewToolRun,
        trace: ToolRunTraceContext,
    ) -> Result<DispatcherToolRunRecord> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin create dispatcher tool run transaction")?;
        let id = insert_tool_run(&tx, &run, &trace)?;
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

    pub fn load_tool_run(&self, id: &str) -> Result<DispatcherToolRunRecord> {
        let conn = self.conn()?;
        load_tool_run_on_conn(&conn, id)
    }
}

// 单调用与批次登记共享同一个事务内插入路径。
pub(super) fn insert_tool_run(
    tx: &rusqlite::Transaction<'_>,
    run: &NewToolRun,
    trace: &ToolRunTraceContext,
) -> Result<String> {
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
            .with_context(|| format!("parent dispatcher tool run not found: {parent_run_id}"))?;
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
    Ok(id)
}

//! 整批登记与身份绑定在一个事务内完成；事务成功前不启动 worker。
use super::{
    lifecycle::insert_tool_run, load_tool_run_on_conn, DispatcherToolRunRecord, NewToolRun,
    ToolRunTraceContext,
};
use crate::agent::db::DispatcherDb;
use anyhow::Result;
use rusqlite::{params, TransactionBehavior};

impl DispatcherDb {
    pub(crate) fn register_tool_task_batch(
        &self,
        tasks: Vec<(NewToolRun, ToolRunTraceContext)>,
        run: &str,
        scope: &str,
        round: u64,
        anchor: &str,
    ) -> Result<Vec<DispatcherToolRunRecord>> {
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut rows = Vec::with_capacity(tasks.len());
        let round = i64::try_from(round)?;
        for (task, trace) in tasks {
            let id = insert_tool_run(&tx, &task, &trace)?;
            tx.execute(
                "UPDATE dispatcher_tool_runs SET agent_run_id=?1, scope_id=?2,
                dispatch_round=?3, root_request_message_id=?4, phase='queued' WHERE id=?5",
                params![run, scope, round, anchor, id],
            )?;
            rows.push(load_tool_run_on_conn(&tx, &id)?);
        }
        tx.commit()?;
        Ok(rows)
    }
}

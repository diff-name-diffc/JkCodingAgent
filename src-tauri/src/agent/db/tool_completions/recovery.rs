//! 应用重启只恢复事实与交付，绝不重放外部副作用。
use super::*;

impl DispatcherDb {
    pub(crate) fn pending_root_tool_completions(
        &self,
        workspace: &str,
    ) -> Result<Vec<ToolCompletion>> {
        let conn = self.conn()?;
        let mut statement = conn.prepare(
            "SELECT c.*, r.tool_name, r.tool_call_id, r.dispatch_round
             FROM dispatcher_tool_completions c JOIN dispatcher_tool_runs r ON r.id=c.tool_run_id
             WHERE r.workspace_id=?1 AND r.parent_run_id IS NULL AND c.observed_request_step IS NULL
             ORDER BY c.event_id",
        )?;
        let rows = statement.query_map([workspace], map_completion)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub(crate) fn recover_interrupted_tool_tasks(&self) -> Result<()> {
        let tasks = {
            let conn = self.conn()?;
            let mut query = conn.prepare(
                "SELECT r.id, r.tool_name, r.started_at IS NOT NULL, r.parent_run_id, r.workspace_id, r.scope_id
                 FROM dispatcher_tool_runs r
                 WHERE r.agent_run_id IS NOT NULL AND r.scope_id IS NOT NULL
                 AND r.status IN ('planned','running')
                 AND NOT EXISTS(SELECT 1 FROM dispatcher_tool_completions c WHERE c.tool_run_id=r.id)
                 ORDER BY r.sequence DESC, r.created_at DESC"
            )?;
            let rows = query.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (task, name, started, parent, workspace, _) in tasks {
            let (kind, message) = if started {
                (
                    "external_state_unknown",
                    "应用已重启，已开始的工具未结算，执行结果未知；禁止自动重跑副作用。",
                )
            } else {
                (
                    "interrupted_not_started",
                    "应用已重启，该工具在实际执行前被中断，未执行。",
                )
            };
            let payload = format!("错误：{message} task_id={task}");
            let event = self.settle_tool_completion(CompletionDraft {
                tool_run_id: task.clone(),
                status: "failed".into(),
                error_kind: Some(kind.into()),
                fatal: false,
                retryable: false,
                display_content: payload.clone(),
                context_payload: payload.clone(),
                result_mode: "raw".into(),
                usage_json: None,
                artifact: ToolArtifactDraft::raw_tool_output(&name, &payload),
            })?;
            if parent.is_none() {
                // 部分批次可能在登记后崩溃，先补齐唯一 tool 应答，再交给新 run 观察。
                let row = self.load_tool_run(&task)?;
                if row.reply_message_id.is_none() {
                    self.reply_to_tool_task(&workspace, &task, Some(event))?;
                }
            } else {
                // 子 scope 的中断由父调用报告；内部事实不混入主对话。
                self.conn()?.execute("UPDATE dispatcher_tool_completions SET observed_request_step=-1 WHERE event_id=?1", [event])?;
            }
        }
        let unpaired = {
            let conn = self.conn()?;
            let mut query = conn.prepare("SELECT r.workspace_id, r.id, c.event_id FROM dispatcher_tool_runs r JOIN dispatcher_tool_completions c ON c.tool_run_id=r.id WHERE r.parent_run_id IS NULL AND r.reply_message_id IS NULL")?;
            let rows = query.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (workspace, task, event) in unpaired {
            self.reply_to_tool_task(&workspace, &task, Some(event))?;
        }
        self.conn()?.execute("UPDATE dispatcher_tool_completions SET observed_request_step=-1 WHERE observed_request_step IS NULL AND tool_run_id IN (SELECT id FROM dispatcher_tool_runs WHERE parent_run_id IS NOT NULL)", [])?;
        Ok(())
    }
}

//! 协调器交付协议：原调用应答一次，迟到结果作为独立 observation。
use anyhow::{ensure, Context, Result};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::json;

use crate::agent::db::{
    content::content_to_segments_json,
    util::{map_dispatcher_message_record, now},
    DispatcherDb, DispatcherMessageRecord,
};

impl DispatcherDb {
    pub(crate) fn observe_internal_completions(
        &self,
        run: &str,
        scope: &str,
        step: i64,
        ids: &[i64],
    ) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for id in ids {
            let changed = tx.execute("UPDATE dispatcher_tool_completions SET observed_request_step = COALESCE(observed_request_step, ?1)
                WHERE event_id = ?2 AND agent_run_id = ?3 AND scope_id = ?4 AND tool_run_id IN
                (SELECT id FROM dispatcher_tool_runs WHERE parent_run_id IS NOT NULL)", params![step, id, run, scope])?;
            ensure!(changed == 1, "不属于子 scope 的完成事件：{id}");
        }
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn set_tool_task_phase(&self, task: &str, phase: &str) -> Result<()> {
        ensure!(
            ["queued", "reviewing", "running", "preparing", "cancelling"].contains(&phase),
            "无效工具阶段"
        );
        self.conn()?.execute("UPDATE dispatcher_tool_runs SET phase=?1 WHERE id=?2 AND status IN ('planned','running')", params![phase, task])?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn bind_tool_task(
        &self,
        task: &str,
        run: &str,
        scope: &str,
        round: u64,
        anchor: &str,
    ) -> Result<()> {
        let conn = self.conn()?;
        let changed = conn.execute("UPDATE dispatcher_tool_runs SET agent_run_id=?1, scope_id=?2,
            dispatch_round=?3, root_request_message_id=?4, phase='queued' WHERE id=?5 AND status='planned'",
            params![run, scope, i64::try_from(round)?, anchor, task])?;
        ensure!(changed == 1, "工具登记状态已改变：{task}");
        Ok(())
    }

    /// actual_event 指定窗口内的实际结果；None 为 accepted。重复调用返回同一消息。
    pub(crate) fn reply_to_tool_task(
        &self,
        workspace_id: &str,
        task_id: &str,
        actual_event: Option<i64>,
    ) -> Result<DispatcherMessageRecord> {
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (call_id, name, phase, reply): (String, String, Option<String>, Option<String>) = tx
            .query_row(
                "SELECT tool_call_id, tool_name, COALESCE(phase, CASE WHEN status NOT IN ('planned','running') THEN 'completed' END), reply_message_id FROM dispatcher_tool_runs
             WHERE id = ?1 AND workspace_id = ?2",
                params![task_id, workspace_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .context("读取工具应答身份")?;
        if let Some(reply) = reply {
            let record = load_message(&tx, &reply)?;
            tx.commit()?;
            return Ok(record);
        }
        let (display, payload, mode): (String, String, String) = match actual_event {
            Some(event_id) => tx.query_row(
                "SELECT display_content, context_payload, result_mode FROM dispatcher_tool_completions
                 WHERE event_id = ?1 AND tool_run_id = ?2 AND delivery_message_id IS NULL",
                params![event_id, task_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).context("读取窗口内工具结果")?,
            None => {
                let phase = phase.context("accepted 任务缺少执行阶段")?;
                let body = json!({"status":"accepted", "task_id":task_id, "state":phase}).to_string();
                (body.clone(), body, "accepted".into())
            }
        };
        let id = insert_message(
            &tx,
            workspace_id,
            "tool",
            task_id,
            Some(&call_id),
            &name,
            &display,
            &payload,
            &mode,
        )?;
        tx.execute("UPDATE dispatcher_tool_runs SET reply_message_id = ?1, dispatch_mode = ?2 WHERE id = ?3",
            params![id, if actual_event.is_some() { "immediate" } else { "accepted" }, task_id])?;
        if let Some(event) = actual_event {
            tx.execute("UPDATE dispatcher_tool_completions SET delivery_message_id = ?1 WHERE event_id = ?2", params![id, event])?;
        }
        let record = load_message(&tx, &id)?;
        tx.commit().context("提交工具唯一应答")?;
        Ok(record)
    }

    /// 只有原批次全部配对后调用；完成事件消息和交付标记原子写入。
    pub(crate) fn deliver_tool_completion(
        &self,
        workspace_id: &str,
        scope_id: &str,
        event_id: i64,
    ) -> Result<DispatcherMessageRecord> {
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (task, call, name, status, display, payload, mode, delivery, reply): (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
        ) = tx
            .query_row(
                "SELECT c.tool_run_id, r.tool_call_id, r.tool_name, c.status, c.display_content,
                c.context_payload, c.result_mode, c.delivery_message_id, r.reply_message_id
             FROM dispatcher_tool_completions c JOIN dispatcher_tool_runs r ON r.id = c.tool_run_id
             WHERE c.event_id = ?1 AND c.scope_id = ?2 AND r.workspace_id = ?3",
                params![event_id, scope_id, workspace_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                    ))
                },
            )
            .context("读取所属 scope 的完成事件")?;
        if let Some(delivery) = delivery {
            let record = load_message(&tx, &delivery)?;
            tx.commit()?;
            return Ok(record);
        }
        ensure!(reply.is_some(), "工具调用尚未应答，禁止插入完成观察");
        let observation = json!({"kind":"tool_completion", "task_id":task,
            "tool_call_id":call, "status":status, "context_payload":payload})
        .to_string();
        let id = insert_message(
            &tx,
            workspace_id,
            "runtime",
            &task,
            None,
            &name,
            &display,
            &observation,
            &mode,
        )?;
        tx.execute(
            "UPDATE dispatcher_tool_completions SET delivery_message_id = ?1 WHERE event_id = ?2",
            params![id, event_id],
        )?;
        let record = load_message(&tx, &id)?;
        tx.commit().context("提交工具完成观察")?;
        Ok(record)
    }

    /// 仅在有效模型响应已提交后，确认本次请求快照确实包含的事件。
    pub(crate) fn observe_tool_completions(
        &self,
        agent_run_id: &str,
        scope_id: &str,
        request_step: i64,
        events: &[i64],
    ) -> Result<()> {
        ensure!(request_step >= 0, "请求序号不能为负数");
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for event in events {
            let delivered: Option<Option<String>> = tx
                .query_row(
                    "SELECT delivery_message_id FROM dispatcher_tool_completions
                 WHERE event_id = ?1 AND agent_run_id = ?2 AND scope_id = ?3",
                    params![event, agent_run_id, scope_id],
                    |row| row.get(0),
                )
                .optional()?;
            ensure!(
                delivered.flatten().is_some(),
                "不能确认未交付或其他 scope 的完成事件：{event}"
            );
            tx.execute("UPDATE dispatcher_tool_completions SET observed_request_step = COALESCE(observed_request_step, ?1) WHERE event_id = ?2", params![request_step, event])?;
        }
        tx.commit().context("确认模型完成观察快照")
    }
}

fn load_message(tx: &Transaction<'_>, id: &str) -> Result<DispatcherMessageRecord> {
    Ok(tx.query_row(
        "SELECT * FROM dispatcher_messages WHERE id = ?1",
        [id],
        map_dispatcher_message_record,
    )?)
}

#[allow(clippy::too_many_arguments)]
fn insert_message(
    tx: &Transaction<'_>,
    workspace: &str,
    role: &str,
    task: &str,
    call: Option<&str>,
    name: &str,
    display: &str,
    payload: &str,
    mode: &str,
) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let timestamp = now();
    tx.execute(
        "INSERT INTO dispatcher_messages
        (id, workspace_id, role, segments_json, context_payload, tool_task_id,
         tool_call_id, tool_name, tool_result_mode, created_at)
        VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            id,
            workspace,
            role,
            content_to_segments_json(display),
            payload,
            task,
            call,
            name,
            mode,
            timestamp
        ],
    )?;
    tx.execute(
        "UPDATE dispatcher_sessions SET updated_at = ?1 WHERE id = ?2",
        params![timestamp, workspace],
    )?;
    Ok(id)
}

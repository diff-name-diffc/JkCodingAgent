//! 主会话与子 scope 共用协调器；仅交付目标和观察持久化方式不同。
use crate::agent::db::{tool_completions::ToolCompletion, DispatcherDb, DispatcherMessageRecord};
use anyhow::Result;
use rig::message::ToolCall;

#[derive(Clone, Copy)]
pub(crate) enum LoopHost {
    Conversation,
    Memory,
}

impl LoopHost {
    pub(crate) async fn reject(
        self,
        db: &DispatcherDb,
        workspace: &str,
        call: &ToolCall,
        text: &str,
    ) -> Result<DispatcherMessageRecord> {
        match self {
            Self::Conversation => {
                db.add_visible_tool_result_async(
                    workspace,
                    text,
                    text,
                    Some(call.wire_call_id()),
                    Some(&call.function.name),
                    Some("raw"),
                    &[],
                )
                .await
            }
            Self::Memory => {
                let mut record = memory_record(
                    workspace,
                    "",
                    "tool",
                    Some(call.wire_call_id()),
                    &call.function.name,
                    text.into(),
                    text.into(),
                    "raw".into(),
                );
                record.tool_task_id = None;
                Ok(record)
            }
        }
    }

    pub(crate) async fn reply(
        self,
        db: &DispatcherDb,
        workspace: &str,
        task: &str,
        call: &ToolCall,
        completion: Option<&ToolCompletion>,
    ) -> Result<DispatcherMessageRecord> {
        match self {
            Self::Conversation => {
                let (db, workspace, task, event) = (
                    db.clone(),
                    workspace.to_string(),
                    task.to_string(),
                    completion.map(|c| c.event_id),
                );
                tokio::task::spawn_blocking(move || db.reply_to_tool_task(&workspace, &task, event))
                    .await?
            }
            Self::Memory => {
                let (display, payload, mode) = match completion {
                    Some(c) => (
                        c.display_content.clone(),
                        c.context_payload.clone(),
                        c.result_mode.clone(),
                    ),
                    None => {
                        let (db, task_id) = (db.clone(), task.to_string());
                        let row = tokio::task::spawn_blocking(move || db.load_tool_run(&task_id))
                            .await??;
                        let phase = row.phase.unwrap_or_else(|| "completed".into());
                        let body =
                            serde_json::json!({"status":"accepted","task_id":task,"state":phase})
                                .to_string();
                        (body.clone(), body, "accepted".into())
                    }
                };
                Ok(memory_record(
                    workspace,
                    task,
                    "tool",
                    Some(call.wire_call_id()),
                    &call.function.name,
                    display,
                    payload,
                    mode,
                ))
            }
        }
    }
    pub(crate) async fn deliver(
        self,
        db: &DispatcherDb,
        workspace: &str,
        scope: &str,
        completion: &ToolCompletion,
    ) -> Result<DispatcherMessageRecord> {
        match self {
            Self::Conversation => {
                let (db, workspace, scope, event) = (
                    db.clone(),
                    workspace.to_string(),
                    scope.to_string(),
                    completion.event_id,
                );
                tokio::task::spawn_blocking(move || {
                    db.deliver_tool_completion(&workspace, &scope, event)
                })
                .await?
            }
            Self::Memory => {
                let payload = serde_json::json!({"kind":"tool_completion", "task_id":completion.tool_run_id,
                    "tool_call_id":completion.tool_call_id, "status":completion.status, "context_payload":completion.context_payload}).to_string();
                Ok(memory_record(
                    workspace,
                    &completion.tool_run_id,
                    "runtime",
                    None,
                    "",
                    completion.display_content.clone(),
                    payload,
                    completion.result_mode.clone(),
                ))
            }
        }
    }
    pub(crate) async fn observe(
        self,
        db: DispatcherDb,
        run: String,
        scope: String,
        step: i64,
        ids: Vec<i64>,
    ) -> Result<()> {
        tokio::task::spawn_blocking(move || match self {
            Self::Conversation => db.observe_tool_completions(&run, &scope, step, &ids),
            Self::Memory => db.observe_internal_completions(&run, &scope, step, &ids),
        })
        .await?
    }
}

fn memory_record(
    workspace: &str,
    task: &str,
    role: &str,
    call: Option<&str>,
    name: &str,
    display: String,
    payload: String,
    mode: String,
) -> DispatcherMessageRecord {
    DispatcherMessageRecord {
        id: uuid::Uuid::new_v4().to_string(),
        workspace_id: workspace.into(),
        role: role.into(),
        segments_json: crate::agent::db::content::content_to_segments_json(&display),
        thinking_content: None,
        thinking_elapsed_ms: None,
        context_payload: Some(payload),
        tool_call_id: call.map(str::to_string),
        tool_task_id: Some(task.into()),
        tool_name: Some(name.into()),
        tool_result_mode: Some(mode),
        tool_artifacts: vec![],
        tool_calls_json: None,
        usage_stats: None,
        created_at: chrono::Utc::now().to_rfc3339(),
    }
}

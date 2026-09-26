use super::*;
use crate::agent::db::NewToolRun;

fn fixture() -> (DispatcherDb, String) {
    let db = DispatcherDb::new(std::env::temp_dir().join(format!(
        "completion-outbox-{}.sqlite3",
        uuid::Uuid::new_v4()
    )))
    .unwrap();
    let run = db
        .create_tool_run(NewToolRun {
            workspace_id: "workspace".into(),
            tool_call_id: "call".into(),
            tool_name: "echo".into(),
            provider: "builtin".into(),
            category: "general".into(),
            arguments_json: "{}".into(),
            effective_arguments_json: "{}".into(),
            metadata_json: "{}".into(),
        })
        .unwrap();
    db.conn().unwrap().execute(
        "UPDATE dispatcher_tool_runs SET agent_run_id = 'root', scope_id = 'child' WHERE id = ?1",
        [&run.id],
    ).unwrap();
    (db, run.id)
}

fn draft(id: &str) -> CompletionDraft {
    CompletionDraft {
        tool_run_id: id.into(),
        status: "succeeded".into(),
        error_kind: None,
        fatal: false,
        retryable: false,
        display_content: "done".into(),
        context_payload: "done".into(),
        result_mode: "raw".into(),
        usage_json: None,
        artifact: ToolArtifactDraft::raw_tool_output("echo", "done"),
    }
}

#[test]
fn settlement_is_idempotent_and_scope_isolated() {
    let (db, id) = fixture();
    let event = db.settle_tool_completion(draft(&id)).unwrap();
    assert_eq!(db.settle_tool_completion(draft(&id)).unwrap(), event);
    assert_eq!(db.load_tool_run(&id).unwrap().status, "succeeded");
    assert!(db
        .pending_tool_completions("root", "parent", i64::MAX)
        .unwrap()
        .is_empty());
    assert!(db
        .pending_tool_completions("root", "child", event - 1)
        .unwrap()
        .is_empty());
    let results = db.pending_tool_completions("root", "child", event).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].context_payload, "done");
    let count: i64 = db
        .conn()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM dispatcher_tool_artifacts WHERE tool_run_id = ?1",
            [&id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn failed_completion_insert_rolls_back_terminal_and_artifact() {
    let (db, id) = fixture();
    db.conn()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER fail_completion BEFORE INSERT ON dispatcher_tool_completions
         BEGIN SELECT RAISE(ABORT, 'injected storage failure'); END;",
        )
        .unwrap();
    assert!(db.settle_tool_completion(draft(&id)).is_err());
    assert_eq!(db.load_tool_run(&id).unwrap().status, "planned");
    let count: i64 = db
        .conn()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM dispatcher_tool_artifacts",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    assert!(db
        .pending_tool_completions("root", "child", i64::MAX)
        .unwrap()
        .is_empty());
}

#[test]
fn truncating_only_notification_preserves_fact_and_redelivers_without_execution() {
    let (db, task) = fixture();
    let request = db
        .add_visible_message_from_segments(
            "workspace",
            "user",
            crate::agent::db::content::content_to_segments_json("执行任务"),
        )
        .unwrap();
    db.bind_tool_task(&task, "root", "child", 1, &request.id)
        .unwrap();
    let reply = db.reply_to_tool_task("workspace", &task, None).unwrap();
    let event = db.settle_tool_completion(draft(&task)).unwrap();
    let notification = db
        .deliver_tool_completion("workspace", "child", event)
        .unwrap();
    db.observe_tool_completions("root", "child", 2, &[event])
        .unwrap();
    db.truncate_messages_from("workspace", &notification.id)
        .unwrap();
    assert_eq!(db.load_tool_run(&task).unwrap().status, "succeeded");
    let pending = db
        .pending_tool_completions("root", "child", i64::MAX)
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].delivery_message_id.is_none());
    assert!(pending[0].observed_request_step.is_none());
    let again = db
        .deliver_tool_completion("workspace", "child", event)
        .unwrap();
    assert_ne!(again.id, notification.id);
    assert_eq!(
        db.reply_to_tool_task("workspace", &task, None).unwrap().id,
        reply.id
    );
    let artifacts: i64 = db
        .conn()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM dispatcher_tool_artifacts WHERE tool_run_id=?1",
            [&task],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(artifacts, 1);
    db.truncate_messages_from("workspace", &request.id).unwrap();
    assert!(db.load_tool_run(&task).is_err());
    assert!(db
        .pending_tool_completions("root", "child", i64::MAX)
        .unwrap()
        .is_empty());
}

#[test]
fn restart_records_interruption_without_replaying_and_preserves_committed_completion() {
    let (db, queued) = fixture();
    db.bind_tool_task(&queued, "root", "child", 1, "anchor")
        .unwrap();
    let running = db
        .create_tool_run(NewToolRun {
            workspace_id: "workspace".into(),
            tool_call_id: "running-call".into(),
            tool_name: "echo".into(),
            provider: "builtin".into(),
            category: "general".into(),
            arguments_json: "{}".into(),
            effective_arguments_json: "{}".into(),
            metadata_json: "{}".into(),
        })
        .unwrap();
    db.bind_tool_task(&running.id, "root", "child", 2, "anchor")
        .unwrap();
    db.mark_tool_run_started(&running.id).unwrap();
    db.recover_interrupted_tool_tasks().unwrap();
    db.recover_interrupted_tool_tasks().unwrap();
    let completions = db.pending_root_tool_completions("workspace").unwrap();
    assert_eq!(completions.len(), 2);
    assert!(completions
        .iter()
        .all(|event| !event.retryable && !event.fatal));
    assert_eq!(
        completions
            .iter()
            .find(|event| event.tool_run_id == queued)
            .unwrap()
            .error_kind
            .as_deref(),
        Some("interrupted_not_started")
    );
    assert_eq!(
        completions
            .iter()
            .find(|event| event.tool_run_id == running.id)
            .unwrap()
            .error_kind
            .as_deref(),
        Some("external_state_unknown")
    );
    assert_eq!(db.count_visible_messages("workspace").unwrap(), 2);
    assert_eq!(db.load_tool_run(&queued).unwrap().status, "failed");
}

//! 工具运行台账回归测试：生命周期单向推进、终态幂等、树语义与级联清理。

use uuid::Uuid;

use super::{DispatcherToolRunRecord, NewToolRun, ToolRunTraceContext, TOOL_RUN_ORIGIN_MODEL};
use crate::agent::db::tool_completions::CompletionDraft;
use crate::agent::db::DispatcherDb;
use crate::agent::db::ToolArtifactDraft;

/// 经生产登记路径种下一条带完整运行身份的工具台账记录。
fn seed_registered_run(db: &DispatcherDb, workspace_id: &str) -> DispatcherToolRunRecord {
    db.register_tool_task_batch(
        vec![(new_run(workspace_id), Default::default())],
        "run",
        "scope",
        1,
        "anchor",
    )
    .expect("register tool run")
    .into_iter()
    .next()
    .expect("registered run row")
}

fn draft(id: &str, status: &str) -> CompletionDraft {
    CompletionDraft {
        tool_run_id: id.into(),
        status: status.into(),
        error_kind: None,
        fatal: false,
        retryable: false,
        display_content: "done".into(),
        context_payload: "done".into(),
        result_mode: "raw".into(),
        usage_json: None,
        artifact: ToolArtifactDraft::raw_tool_output("demo_tool", "done"),
    }
}

fn test_db() -> DispatcherDb {
    let path = std::env::temp_dir().join(format!(
        "jkcodingagent-tool-runs-{}.sqlite3",
        Uuid::new_v4()
    ));
    DispatcherDb::new(path).expect("create test dispatcher db")
}

fn new_run(workspace_id: &str) -> NewToolRun {
    NewToolRun {
        workspace_id: workspace_id.to_string(),
        tool_call_id: format!("call-{}", Uuid::new_v4()),
        tool_name: "demo_tool".to_string(),
        provider: "builtin".to_string(),
        category: "general".to_string(),
        arguments_json: "{}".to_string(),
        effective_arguments_json: "{}".to_string(),
        metadata_json: "{}".to_string(),
    }
}

#[test]
fn batch_registration_rolls_back_all_rows_if_a_later_parent_is_invalid() {
    let db = test_db();
    let first = new_run("ws");
    let call = first.tool_call_id.clone();
    let invalid = ToolRunTraceContext {
        parent_run_id: Some("missing".into()),
        ..Default::default()
    };
    assert!(db
        .register_tool_task_batch(
            vec![(first, Default::default()), (new_run("ws"), invalid)],
            "run",
            "scope",
            1,
            "anchor"
        )
        .is_err());
    assert!(db
        .list_tool_run_tree_for_call("ws", &call, None)
        .unwrap()
        .is_empty());
}

#[test]
fn batch_registration_commits_all_runtime_identity_fields() {
    let db = test_db();
    let rows = db
        .register_tool_task_batch(
            vec![
                (new_run("ws"), Default::default()),
                (new_run("ws"), Default::default()),
            ],
            "run",
            "scope",
            2,
            "anchor",
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(row.agent_run_id.as_deref(), Some("run"));
        assert_eq!(row.scope_id.as_deref(), Some("scope"));
        assert_eq!(row.dispatch_round, Some(2));
        assert_eq!(row.root_request_message_id.as_deref(), Some("anchor"));
        assert_eq!(row.phase.as_deref(), Some("queued"));
    }
}

#[test]
fn settlement_advances_lifecycle_and_records_duration() {
    let db = test_db();
    let run = seed_registered_run(&db, "ws");
    assert_eq!(run.status, "planned");
    assert_eq!(run.phase.as_deref(), Some("queued"));

    let started = db.mark_tool_run_started(&run.id).expect("start run");
    assert_eq!(started.status, "running");
    assert!(started.started_at.is_some());

    db.settle_tool_completion(draft(&run.id, "succeeded"))
        .expect("settle run");
    let finished = db.load_tool_run(&run.id).expect("reload run");
    assert_eq!(finished.status, "succeeded");
    assert!(finished.finished_at.is_some());
    assert!(finished.started_at.is_some());
}

#[test]
fn started_does_not_regress_terminal_state() {
    let db = test_db();
    let run = seed_registered_run(&db, "ws");
    db.mark_tool_run_started(&run.id).expect("start run");
    db.settle_tool_completion(draft(&run.id, "succeeded"))
        .expect("settle run");

    let regressed = db.mark_tool_run_started(&run.id).expect("re-start no-op");
    assert_eq!(regressed.status, "succeeded", "终态不得回退到 running");
}

#[test]
fn duration_is_nonnegative_even_with_missing_started_at() {
    // 直接结算一个 planned（未 started）的 run，时长应容错为 0 而非 NULL。
    let db = test_db();
    let run = seed_registered_run(&db, "ws");
    db.settle_tool_completion(draft(&run.id, "cancelled"))
        .expect("settle planned run");
    let finished = db.load_tool_run(&run.id).expect("reload run");
    assert_eq!(finished.duration_ms, 0);
    assert!(finished.started_at.is_none());
}

#[test]
fn traced_runs_round_trip_and_tree_is_depth_first() {
    let db = test_db();
    let root = db.create_tool_run(new_run("ws")).expect("create root");
    assert_eq!(root.parent_run_id, None);
    assert_eq!(root.origin, TOOL_RUN_ORIGIN_MODEL);
    assert_eq!(root.step_id, None);
    assert_eq!(root.sequence, 0);

    let first = db
        .create_tool_run_with_trace(
            new_run("ws"),
            ToolRunTraceContext {
                parent_run_id: Some(root.id.clone()),
                origin: "tool_program".to_string(),
                step_id: Some("search".to_string()),
                sequence: 0,
            },
        )
        .expect("create first child");
    let grandchild = db
        .create_tool_run_with_trace(
            new_run("ws"),
            ToolRunTraceContext {
                parent_run_id: Some(first.id.clone()),
                origin: "tool_program".to_string(),
                step_id: Some("read".to_string()),
                sequence: 0,
            },
        )
        .expect("create grandchild");
    let second = db
        .create_tool_run_with_trace(
            new_run("ws"),
            ToolRunTraceContext {
                parent_run_id: Some(root.id.clone()),
                origin: "tool_program".to_string(),
                step_id: Some("summarize".to_string()),
                sequence: 1,
            },
        )
        .expect("create second child");

    assert_eq!(first.parent_run_id.as_deref(), Some(root.id.as_str()));
    assert_eq!(first.origin, "tool_program");
    assert_eq!(first.step_id.as_deref(), Some("search"));
    assert_eq!(first.sequence, 0);

    let tree = db
        .list_tool_run_tree("ws", &root.id)
        .expect("list tool run tree");
    assert_eq!(
        tree.iter().map(|run| run.id.as_str()).collect::<Vec<_>>(),
        vec![
            root.id.as_str(),
            first.id.as_str(),
            grandchild.id.as_str(),
            second.id.as_str()
        ]
    );
}

#[test]
fn tree_for_call_selects_only_the_requested_root() {
    let db = test_db();
    let mut first_run = new_run("ws");
    first_run.tool_call_id = "shared-call".to_string();
    let first = db.create_tool_run(first_run).expect("create first root");
    let child = db
        .create_tool_run_with_trace(
            new_run("ws"),
            ToolRunTraceContext {
                parent_run_id: Some(first.id.clone()),
                origin: "tool_program".to_string(),
                step_id: Some("read".to_string()),
                sequence: 1,
            },
        )
        .expect("create child");
    let mut unrelated_run = new_run("other-ws");
    unrelated_run.tool_call_id = "shared-call".to_string();
    db.create_tool_run(unrelated_run)
        .expect("create unrelated root");

    let tree = db
        .list_tool_run_tree_for_call("ws", "shared-call", None)
        .expect("load by tool call");
    assert_eq!(
        tree.iter().map(|run| run.id.as_str()).collect::<Vec<_>>(),
        vec![first.id.as_str(), child.id.as_str()]
    );

    let explicit = db
        .list_tool_run_tree_for_call("ws", "ignored", Some(&first.id))
        .expect("load by root id");
    assert_eq!(explicit.len(), 2);
    assert!(db
        .list_tool_run_tree_for_call("other-ws", "ignored", Some(&first.id))
        .expect("cross-workspace root is invisible")
        .is_empty());
}

#[test]
fn traced_run_rejects_cross_workspace_parent_and_duplicate_sequence() {
    let db = test_db();
    let root = db.create_tool_run(new_run("ws-a")).expect("create root");

    let cross_workspace = db
        .create_tool_run_with_trace(
            new_run("ws-b"),
            ToolRunTraceContext {
                parent_run_id: Some(root.id.clone()),
                origin: "tool_program".to_string(),
                step_id: None,
                sequence: 0,
            },
        )
        .expect_err("cross-workspace parent must fail");
    assert!(cross_workspace.to_string().contains("belongs to workspace"));

    let trace = ToolRunTraceContext {
        parent_run_id: Some(root.id.clone()),
        origin: "tool_program".to_string(),
        step_id: Some("step".to_string()),
        sequence: 0,
    };
    db.create_tool_run_with_trace(new_run("ws-a"), trace.clone())
        .expect("create first child");
    let duplicate = db
        .create_tool_run_with_trace(new_run("ws-a"), trace)
        .expect_err("duplicate sibling sequence must fail");
    assert!(duplicate.to_string().contains("create dispatcher tool run"));
}

#[test]
fn failed_and_internal_error_are_terminal() {
    let db = test_db();
    for status in ["failed", "internal_error"] {
        let run = seed_registered_run(&db, "ws");
        db.mark_tool_run_started(&run.id).expect("start run");
        db.settle_tool_completion(draft(&run.id, status))
            .expect("settle run");
        assert_eq!(db.load_tool_run(&run.id).unwrap().status, status);

        // 终态后再结算同一运行：幂等返回首个事件，状态不被改写。
        db.settle_tool_completion(draft(&run.id, "succeeded"))
            .expect("repeat settle is a no-op");
        assert_eq!(db.load_tool_run(&run.id).unwrap().status, status);
    }
}

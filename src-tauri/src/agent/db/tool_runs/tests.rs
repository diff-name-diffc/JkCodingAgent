//! 工具运行台账回归测试：生命周期单向推进、终态幂等、树语义与级联清理。

use rusqlite::params;
use uuid::Uuid;

use super::{FinishToolRun, NewToolRun, ToolRunTraceContext, TOOL_RUN_ORIGIN_MODEL};
use crate::agent::db::DispatcherDb;

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

fn finish(status: &str) -> FinishToolRun {
    FinishToolRun {
        status: status.to_string(),
        ..Default::default()
    }
}

#[test]
fn finish_advances_lifecycle_and_records_duration() {
    let db = test_db();
    let run = db.create_tool_run(new_run("ws")).expect("create run");
    assert_eq!(run.status, "planned");

    let started = db.mark_tool_run_started(&run.id).expect("start run");
    assert_eq!(started.status, "running");
    assert!(started.started_at.is_some());

    let finished = db
        .finish_tool_run(&run.id, finish("succeeded"))
        .expect("finish run");
    assert_eq!(finished.status, "succeeded");
    assert!(finished.finished_at.is_some());
    assert!(finished.started_at.is_some());
}

#[test]
fn terminal_state_is_not_overwritten_by_second_finish() {
    let db = test_db();
    let run = db.create_tool_run(new_run("ws")).expect("create run");
    db.mark_tool_run_started(&run.id).expect("start run");

    let first = db
        .finish_tool_run(
            &run.id,
            FinishToolRun {
                status: "recoverable_error".to_string(),
                error_kind: Some("retryable".to_string()),
                error_message: Some("boom".to_string()),
                ..Default::default()
            },
        )
        .expect("first finish");
    assert_eq!(first.status, "recoverable_error");
    assert_eq!(first.error_message.as_deref(), Some("boom"));

    // 重复/乱序 finish 不得把终态改回或清空错误信息。
    let second = db
        .finish_tool_run(
            &run.id,
            FinishToolRun {
                status: "succeeded".to_string(),
                error_kind: None,
                error_message: None,
                ..Default::default()
            },
        )
        .expect("second finish is a no-op");
    assert_eq!(second.status, "recoverable_error", "终态不得被覆盖");
    assert_eq!(
        second.error_message.as_deref(),
        Some("boom"),
        "错误信息不得被清空"
    );
}

#[test]
fn started_does_not_regress_terminal_state() {
    let db = test_db();
    let run = db.create_tool_run(new_run("ws")).expect("create run");
    db.mark_tool_run_started(&run.id).expect("start run");
    db.finish_tool_run(&run.id, finish("succeeded"))
        .expect("finish run");

    let regressed = db.mark_tool_run_started(&run.id).expect("re-start no-op");
    assert_eq!(regressed.status, "succeeded", "终态不得回退到 running");
}

#[test]
fn finish_missing_run_errors() {
    let db = test_db();
    let error = db
        .finish_tool_run("no-such-run", finish("succeeded"))
        .expect_err("missing run must fail");
    assert!(error.to_string().contains("not found"));
}

#[test]
fn duration_is_nonnegative_even_with_missing_started_at() {
    // 直接 finish 一个 planned（未 started）的 run，时长应容错为 0 而非 NULL。
    let db = test_db();
    let run = db.create_tool_run(new_run("ws")).expect("create run");
    let finished = db
        .finish_tool_run(&run.id, finish("cancelled"))
        .expect("finish planned run");
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
fn deleting_parent_cascades_to_descendants() {
    let db = test_db();
    let root = db.create_tool_run(new_run("ws")).expect("create root");
    let child = db
        .create_tool_run_with_trace(
            new_run("ws"),
            ToolRunTraceContext {
                parent_run_id: Some(root.id.clone()),
                origin: "tool_program".to_string(),
                step_id: None,
                sequence: 0,
            },
        )
        .expect("create child");

    assert!(db
        .delete_unattached_tool_run_tree("other-ws", &root.id)
        .expect_err("cross-workspace delete must fail")
        .to_string()
        .contains("not found"));
    db.delete_unattached_tool_run_tree("ws", &root.id)
        .expect("delete parent tree");
    let conn = db.conn().expect("db conn");
    let remaining: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM dispatcher_tool_runs WHERE id IN (?1, ?2)",
            params![&root.id, &child.id],
            |row| row.get(0),
        )
        .expect("count remaining runs");
    assert_eq!(remaining, 0);
}

#[test]
fn failed_and_internal_error_are_terminal() {
    let db = test_db();
    for status in ["failed", "internal_error"] {
        let run = db.create_tool_run(new_run("ws")).expect("create run");
        db.mark_tool_run_started(&run.id).expect("start run");
        let terminal = db
            .finish_tool_run(&run.id, finish(status))
            .expect("finish run");
        assert_eq!(terminal.status, status);

        let unchanged = db
            .finish_tool_run(&run.id, finish("succeeded"))
            .expect("second finish is no-op");
        assert_eq!(unchanged.status, status);
    }
}

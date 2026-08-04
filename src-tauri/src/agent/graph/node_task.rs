//! PI 节点生命周期：持久化运行快照、发事件、执行并返回调度结果。

use std::collections::HashSet;

use futures::FutureExt;
use serde_json::{Map, Value};
use tauri::AppHandle;

use super::node_exec::{execute_node, NodeExecContext, NodeExecOutcome};
use super::runner::emit_run_event;
use super::store::GraphStore;
use super::types::{
    GraphNode, GraphNodeRunRecord, GraphRunEvent, NODE_CANCELLED, NODE_RUNNING, NODE_SKIPPED,
};

pub(super) struct NodeTaskContext {
    pub exec: NodeExecContext,
    pub store: GraphStore,
    pub input: String,
}
pub(super) struct NodeTaskResult {
    pub node_id: String,
    pub output_key: String,
    pub record: GraphNodeRunRecord,
    pub outcome: NodeExecOutcome,
    pub duration_ms: u64,
}

pub(super) async fn run_node_task(ctx: NodeTaskContext) -> NodeTaskResult {
    let node_id = ctx.exec.node.id.clone();
    let output_key = ctx.exec.node.output_key.clone();
    let started = chrono::Utc::now().timestamp_millis();
    let mut record =
        GraphNodeRunRecord::pending(&ctx.exec.run_id, &ctx.exec.plan_id, &ctx.exec.node);
    record.status = NODE_RUNNING.into();
    record.phase = "starting".into();
    record.input_text = ctx.input.clone();
    record.model_label = ctx.exec.harness.model_label.clone();
    record.model_category = ctx.exec.harness.model.category.clone();
    record.started_at = Some(started);
    if let Err(error) = ctx.store.save_node_run_async(&record).await {
        eprintln!("[graph] 保存节点运行记录失败：{error:#}")
    }
    emit_run_event(
        &ctx.exec.app,
        &ctx.exec.plan_id,
        &ctx.exec.run_id,
        &ctx.exec.workspace_id,
        GraphRunEvent::NodeStarted {
            node_id: node_id.clone(),
            title: ctx.exec.node.title.clone(),
            model_ref: ctx.exec.node.model_ref.clone(),
            model_label: ctx.exec.harness.model_label.clone(),
            input: ctx.input.clone(),
        },
    );
    let outcome = std::panic::AssertUnwindSafe(execute_node(&ctx.exec))
        .catch_unwind()
        .await
        .unwrap_or_else(|error| NodeExecOutcome::Failed(format!("节点执行内部错误：{error:?}")));
    let finished = chrono::Utc::now().timestamp_millis();
    record.finished_at = Some(finished);
    record.duration_ms = Some((finished - started).max(0));
    record.phase = "finalizing".into();
    NodeTaskResult {
        node_id,
        output_key,
        record,
        outcome,
        duration_ms: (finished - started).max(0) as u64,
    }
}

pub(super) async fn finish_node_record(
    store: &GraphStore,
    mut record: GraphNodeRunRecord,
    status: &str,
    output: Option<&str>,
    error: Option<&str>,
) {
    record.status = status.into();
    if let Some(v) = output {
        record.output_text = v.into()
    }
    if let Some(v) = error {
        record.error_text = Some(v.into())
    }
    if let Err(error) = store.save_node_run_async(&record).await {
        eprintln!("[graph] 保存节点终态失败：{error:#}")
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn persist_and_emit_state(
    app: &AppHandle,
    store: &GraphStore,
    plan_id: &str,
    run_id: &str,
    workspace_id: &str,
    node_id: &str,
    key: &str,
    value: &str,
    state: &Map<String, Value>,
) {
    let state_json = Value::Object(state.clone()).to_string();
    if let Err(error) = store.update_plan_state_async(plan_id, &state_json).await {
        eprintln!("[graph] 持久化共享状态失败：{error:#}")
    }
    emit_run_event(
        app,
        plan_id,
        run_id,
        workspace_id,
        GraphRunEvent::StateUpdated {
            node_id: node_id.into(),
            key: key.into(),
            value: value.into(),
            state: Value::Object(state.clone()),
        },
    );
}

pub(super) async fn mark_node_skipped(
    app: &AppHandle,
    store: &GraphStore,
    plan_id: &str,
    run_id: &str,
    workspace_id: &str,
    node: &GraphNode,
    reason: &str,
) {
    let mut record = GraphNodeRunRecord::pending(run_id, plan_id, node);
    record.status = NODE_SKIPPED.into();
    record.error_text = Some(reason.into());
    if let Err(error) = store.save_node_run_async(&record).await {
        eprintln!("[graph] 保存跳过节点记录失败（{}）：{error:#}", node.id);
    }
    emit_run_event(
        app,
        plan_id,
        run_id,
        workspace_id,
        GraphRunEvent::NodeSkipped {
            node_id: node.id.clone(),
            reason: reason.into(),
        },
    );
}

async fn mark_node_cancelled(
    app: &AppHandle,
    store: &GraphStore,
    plan_id: &str,
    run_id: &str,
    workspace_id: &str,
    node: &GraphNode,
) {
    let mut record = GraphNodeRunRecord::pending(run_id, plan_id, node);
    record.status = NODE_CANCELLED.into();
    record.error_text = Some("运行已取消".into());
    if let Err(error) = store.save_node_run_async(&record).await {
        eprintln!("[graph] 保存取消节点记录失败（{}）：{error:#}", node.id);
    }
    emit_run_event(
        app,
        plan_id,
        run_id,
        workspace_id,
        GraphRunEvent::NodeSkipped {
            node_id: node.id.clone(),
            reason: "运行已取消".into(),
        },
    );
}

pub(super) async fn cancel_pending_nodes(
    app: &AppHandle,
    store: &GraphStore,
    plan_id: &str,
    run_id: &str,
    workspace_id: &str,
    nodes: &[GraphNode],
) {
    let existing = store.list_node_runs_async(run_id).await.unwrap_or_default();
    let settled = existing
        .iter()
        .filter(|run| run.status != "pending")
        .map(|run| run.node_id.as_str())
        .collect::<HashSet<_>>();
    for node in nodes {
        if !settled.contains(node.id.as_str()) {
            mark_node_cancelled(app, store, plan_id, run_id, workspace_id, node).await
        }
    }
}

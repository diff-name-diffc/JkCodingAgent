//! 节点任务生命周期：置 running（落库 + nodeStarted）→ 执行 → 结果落库与事件。
//! 被 runner 的分层调度以 JoinSet 并行驱动；panic 由 catch_unwind 兜底转为失败，
//! 保证 JoinSet 总能拿到结果。

use std::collections::HashSet;

use futures::FutureExt;
use serde_json::{Map, Value};
use tauri::AppHandle;

use super::node_exec::{execute_node, graph_node_tool_call_id, NodeExecContext, NodeExecOutcome};
use super::runner::emit_run_event;
use super::store::GraphStore;
use super::types::{
    GraphNode, GraphNodeAgent, GraphNodeRunRecord, GraphRunEvent, NODE_CANCELLED, NODE_RUNNING,
    NODE_SKIPPED,
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

/// 单个节点的完整生命周期：置 running（落库 + nodeStarted）→ 执行 → 返回结果。
pub(super) async fn run_node_task(ctx: NodeTaskContext) -> NodeTaskResult {
    let node_id = ctx.exec.node.id.clone();
    let output_key = ctx.exec.node.output_key.clone();
    let plan_id = ctx.exec.plan_id.clone();
    let workspace_id = ctx.exec.workspace_id.clone();
    let started_at = chrono::Utc::now().timestamp_millis();

    let trace_tool_call_id = match &ctx.exec.node.agent {
        GraphNodeAgent::SubAgent { .. } => Some(graph_node_tool_call_id(&plan_id, &node_id)),
        _ => None,
    };
    let mut record = GraphNodeRunRecord::pending(&plan_id, &ctx.exec.node);
    record.status = NODE_RUNNING.to_string();
    record.input_text = ctx.input.clone();
    record.trace_tool_call_id = trace_tool_call_id;
    record.started_at = Some(started_at);
    if let Err(error) = ctx.store.save_node_run_async(&record).await {
        eprintln!("[graph] 保存节点运行记录失败（{plan_id}/{node_id}）：{error:#}");
    }
    emit_run_event(
        &ctx.exec.app,
        &plan_id,
        &workspace_id,
        GraphRunEvent::NodeStarted {
            node_id: node_id.clone(),
            title: ctx.exec.node.title.clone(),
            agent_kind: ctx.exec.node.agent.kind_str().to_string(),
            agent_id: ctx.exec.node.agent.agent_id().map(str::to_string),
            input: ctx.input.clone(),
        },
    );

    let outcome = std::panic::AssertUnwindSafe(execute_node(&ctx.exec))
        .catch_unwind()
        .await
        .unwrap_or_else(|error| NodeExecOutcome::Failed(format!("节点执行内部错误：{error:?}")));

    let finished_at = chrono::Utc::now().timestamp_millis();
    record.finished_at = Some(finished_at);
    record.duration_ms = Some((finished_at - started_at).max(0));

    NodeTaskResult {
        node_id,
        output_key,
        record,
        outcome,
        duration_ms: (finished_at - started_at).max(0) as u64,
    }
}

pub(super) async fn finish_node_record(
    store: &GraphStore,
    mut record: GraphNodeRunRecord,
    status: &str,
    output: Option<&str>,
    error: Option<&str>,
) {
    record.status = status.to_string();
    if let Some(output) = output {
        record.output_text = output.to_string();
    }
    if let Some(error) = error {
        record.error_text = Some(error.to_string());
    }
    if let Err(error) = store.save_node_run_async(&record).await {
        eprintln!(
            "[graph] 保存节点运行记录失败（{}/{}）：{error:#}",
            record.plan_id, record.node_id
        );
    }
}

/// 共享 state 持久化 + stateUpdated 事件。
#[allow(clippy::too_many_arguments)]
pub(super) async fn persist_and_emit_state(
    app: &AppHandle,
    store: &GraphStore,
    plan_id: &str,
    workspace_id: &str,
    node_id: &str,
    key: &str,
    value: &str,
    state: &Map<String, Value>,
) {
    let state_json = Value::Object(state.clone()).to_string();
    if let Err(error) = store.update_plan_state_async(plan_id, &state_json).await {
        eprintln!("[graph] 持久化共享状态失败（{plan_id}）：{error:#}");
    }
    emit_run_event(
        app,
        plan_id,
        workspace_id,
        GraphRunEvent::StateUpdated {
            node_id: node_id.to_string(),
            key: key.to_string(),
            value: value.to_string(),
            state: Value::Object(state.clone()),
        },
    );
}

pub(super) async fn mark_node_skipped(
    app: &AppHandle,
    store: &GraphStore,
    plan_id: &str,
    workspace_id: &str,
    node: &GraphNode,
    reason: &str,
) {
    let mut record = GraphNodeRunRecord::pending(plan_id, node);
    record.status = NODE_SKIPPED.to_string();
    record.error_text = Some(reason.to_string());
    if let Err(error) = store.save_node_run_async(&record).await {
        eprintln!("[graph] 保存节点跳过记录失败（{plan_id}/{}）：{error:#}", node.id);
    }
    emit_run_event(
        app,
        plan_id,
        workspace_id,
        GraphRunEvent::NodeSkipped {
            node_id: node.id.clone(),
            reason: reason.to_string(),
        },
    );
}

async fn mark_node_cancelled(
    app: &AppHandle,
    store: &GraphStore,
    plan_id: &str,
    workspace_id: &str,
    node: &GraphNode,
    reason: &str,
) {
    let mut record = GraphNodeRunRecord::pending(plan_id, node);
    record.status = NODE_CANCELLED.to_string();
    record.error_text = Some(reason.to_string());
    if let Err(error) = store.save_node_run_async(&record).await {
        eprintln!("[graph] 保存节点取消记录失败（{plan_id}/{}）：{error:#}", node.id);
    }
    // 事件词汇表内无 nodeCancelled：以 nodeSkipped + 取消原因表达。
    emit_run_event(
        app,
        plan_id,
        workspace_id,
        GraphRunEvent::NodeSkipped {
            node_id: node.id.clone(),
            reason: reason.to_string(),
        },
    );
}

/// 取消时把仍处于 pending 的节点全部标记 cancelled（running/failed/skipped 等已落定的不动）。
pub(super) async fn cancel_pending_nodes(
    app: &AppHandle,
    store: &GraphStore,
    plan_id: &str,
    workspace_id: &str,
    nodes: &[GraphNode],
) {
    let existing = store.list_node_runs_async(plan_id).await.unwrap_or_default();
    let settled: HashSet<&str> = existing
        .iter()
        .filter(|run| run.status != "pending")
        .map(|run| run.node_id.as_str())
        .collect();
    for node in nodes {
        if settled.contains(node.id.as_str()) {
            continue;
        }
        mark_node_cancelled(app, store, plan_id, workspace_id, node, "运行已取消").await;
    }
}

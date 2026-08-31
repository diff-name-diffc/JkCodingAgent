//! PI 节点生命周期：持久化运行快照、发事件、执行并返回调度结果。

use std::collections::HashSet;

use futures::FutureExt;
use serde_json::{Map, Value};
use tauri::AppHandle;

use super::node_exec::{execute_node, NodeExecContext, NodeExecOutcome};
use super::runner::emit_run_event;
use super::store::GraphStore;
use super::types::{
    GraphNode, GraphNodeRunRecord, GraphRunEvent, NODE_CANCELLED, NODE_PENDING,
    NODE_PHASE_FINALIZING, NODE_PHASE_STARTING, NODE_RUNNING, NODE_SKIPPED,
};

pub(super) struct NodeTaskContext {
    pub exec: NodeExecContext,
    pub store: GraphStore,
    pub input: String,
    /// 当前是第几次重试（0=首次执行）。重试时输入已注入上次失败原因。
    pub retry_count: i32,
}
pub(super) struct NodeTaskResult {
    pub node_id: String,
    pub output_key: String,
    pub record: GraphNodeRunRecord,
    pub outcome: NodeExecOutcome,
    pub duration_ms: i64,
}

pub(super) async fn run_node_task(ctx: NodeTaskContext) -> NodeTaskResult {
    let node_id = ctx.exec.node.id.clone();
    let output_key = ctx.exec.node.output_key.clone();
    let started = chrono::Utc::now().timestamp_millis();
    let mut record =
        GraphNodeRunRecord::pending(&ctx.exec.run_id, &ctx.exec.plan_id, &ctx.exec.node);
    record.status = NODE_RUNNING.into();
    record.phase = NODE_PHASE_STARTING.into();
    record.input_text = ctx.input.clone();
    record.model_label = ctx.exec.harness.model_label.clone();
    record.model_category = ctx.exec.harness.model.category.clone();
    record.started_at = Some(started);
    record.retry_count = ctx.retry_count;
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
        .unwrap_or_else(|error| NodeExecOutcome::Failed {
            error: format!("节点执行内部错误：{error:?}"),
            usage_json: "{}".into(),
        });
    let finished = chrono::Utc::now().timestamp_millis();
    record.finished_at = Some(finished);
    record.duration_ms = Some((finished - started).max(0));
    record.phase = NODE_PHASE_FINALIZING.into();
    NodeTaskResult {
        node_id,
        output_key,
        record,
        outcome,
        duration_ms: (finished - started).max(0),
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
) -> anyhow::Result<()> {
    let state_json = Value::Object(state.clone()).to_string();
    // fail-closed：DB 写失败向上传播，仅在写库成功后广播 StateUpdated——
    // 否则前端事件状态与 DB 持久化漂移且无告警。
    store.update_plan_state_async(plan_id, &state_json).await?;
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
    Ok(())
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
    // 仅在落库成功后广播事件，避免前端状态与 DB 漂移。
    match store.save_node_run_async(&record).await {
        Ok(()) => {
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
        Err(error) => {
            eprintln!("[graph] 保存跳过节点记录失败（{}）：{error:#}", node.id);
        }
    }
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
    // 仅在落库成功后广播事件，避免前端状态与 DB 漂移。
    // 取消与跳过语义不同：发 NodeCancelled（而非复用 NodeSkipped），
    // 前端据此区分「被取消」与「上游失败被跳过」。
    match store.save_node_run_async(&record).await {
        Ok(()) => {
            emit_run_event(
                app,
                plan_id,
                run_id,
                workspace_id,
                GraphRunEvent::NodeCancelled {
                    node_id: node.id.clone(),
                },
            );
        }
        Err(error) => {
            eprintln!("[graph] 保存取消节点记录失败（{}）：{error:#}", node.id);
        }
    }
}

pub(super) async fn cancel_pending_nodes(
    app: &AppHandle,
    store: &GraphStore,
    plan_id: &str,
    run_id: &str,
    workspace_id: &str,
    nodes: &[GraphNode],
) -> anyhow::Result<()> {
    // fail-closed：读取既有节点记录失败时返回错误并跳过取消（保留未确认节点
    // 的原状态）。绝不能 unwrap_or_default 当空集——那会让 settled 为空，
    // mark_node_cancelled 的 INSERT OR REPLACE 将把所有节点（含已成功/失败/
    // 已取消的终态记录）覆盖成 pending 重建的 cancelled 行，丢失产出与用量。
    let existing = store.list_node_runs_async(run_id).await?;
    // 仅取消状态【显式为 pending】的节点：running/终态一律不覆盖。
    // 与旧实现（status != "pending" 即视为已结算）相比，停留在 running 的
    // 异常记录不再被静默放过也不被覆盖，由 fail_interrupted_runs 兜底。
    let pending = existing
        .iter()
        .filter(|run| run.status == NODE_PENDING)
        .map(|run| run.node_id.clone())
        .collect::<HashSet<_>>();
    for node in nodes {
        if pending.contains(&node.id) {
            mark_node_cancelled(app, store, plan_id, run_id, workspace_id, node).await
        }
    }
    Ok(())
}

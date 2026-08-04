//! PI 执行图调度器：拓扑分层、同层最多三节点并行、失败仅阻断下游。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde_json::{Map, Value};
use tauri::{AppHandle, Emitter};
use tokio::sync::watch;
use tokio::task::JoinSet;

use super::harness::{build_harness_catalog, resolve_node_harness};
use super::input::assemble_node_input;
use super::node_exec::{NodeExecContext, NodeExecOutcome};
use super::node_task::{
    cancel_pending_nodes, finish_node_record, mark_node_skipped, persist_and_emit_state,
    run_node_task, NodeTaskContext, NodeTaskResult,
};
use super::store::GraphStore;
use super::types::{
    GraphDefinition, GraphNodeRunRecord, GraphPlanRecord, GraphPlanUpdatedPayload, GraphRunEvent,
    GraphRunEventPayload, GraphRunSummary, NODE_CANCELLED, NODE_FAILED, NODE_SUCCEEDED,
    PLAN_CANCELLED, PLAN_COMPLETED, PLAN_FAILED, STATE_VALUE_MAX_CHARS,
};
use super::validate::{topological_layers, validate_graph};
use crate::agent::agents::project::resolve_project_chat_provider;
use crate::agent::config::DispatcherAgentConfig;
use crate::agent::db::DispatcherDb;
use crate::agent::tools::{ToolContext, ToolRegistry};
use crate::project::mcp::ProjectMcpRegistry;
use crate::shared::truncate_for_display;
use crate::ssh_tool::SshSessionManager;

const MAX_PARALLEL_NODES: usize = 3;
const STATE_TRUNCATE_SUFFIX: &str = "\n...[输出已截断]";
static EVENT_SEQUENCE: AtomicI64 = AtomicI64::new(0);

pub(crate) struct GraphRunServices {
    pub db: DispatcherDb,
    pub agent_config: DispatcherAgentConfig,
    pub project_mcp_registry: ProjectMcpRegistry,
    pub ssh_manager: SshSessionManager,
}

pub(crate) fn emit_run_event(
    app: &AppHandle,
    plan_id: &str,
    run_id: &str,
    workspace_id: &str,
    event: GraphRunEvent,
) {
    let payload = GraphRunEventPayload {
        plan_id: plan_id.into(),
        run_id: run_id.into(),
        workspace_id: workspace_id.into(),
        sequence: EVENT_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1,
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        event,
    };
    let _ = app.emit("graph-run-event", payload);
}
pub(crate) fn emit_plan_updated(app: &AppHandle, plan_id: &str, workspace_id: &str) {
    let _ = app.emit(
        "graph-plan-updated",
        GraphPlanUpdatedPayload {
            plan_id: plan_id.into(),
            workspace_id: workspace_id.into(),
        },
    );
}

pub(crate) async fn execute_graph_run(
    app: AppHandle,
    services: GraphRunServices,
    plan_id: String,
    cancel_rx: watch::Receiver<bool>,
) {
    let store = GraphStore::new(&services.db);
    let Some(plan) = store.get_plan_async(&plan_id).await.ok().flatten() else {
        eprintln!("[graph] 图计划不存在：{plan_id}");
        return;
    };
    let workspace_id = plan.workspace_id.clone();
    let run = match store.create_run_async(&plan_id).await {
        Ok(run) => run,
        Err(error) => {
            eprintln!("[graph] 创建运行失败：{error:#}");
            let _ = store.update_plan_status_async(&plan_id, PLAN_FAILED).await;
            emit_plan_updated(&app, &plan_id, &workspace_id);
            return;
        }
    };
    if let Err(error) = run_graph(&app, &services, &store, plan, run.clone(), cancel_rx).await {
        let message = format!("{error:#}");
        eprintln!("[graph] 图运行失败（{plan_id}）：{message}");
        if let Err(error) = store.fail_interrupted_runs_async(Some(&plan_id)).await {
            eprintln!("[graph] 恢复中断运行状态失败（{plan_id}）：{error:#}");
        }
        let _ = store.finish_run_async(&run.id, PLAN_FAILED).await;
        let _ = store.update_plan_status_async(&plan_id, PLAN_FAILED).await;
        emit_plan_updated(&app, &plan_id, &workspace_id);
        emit_run_event(
            &app,
            &plan_id,
            &run.id,
            &workspace_id,
            GraphRunEvent::RunFailed { error: message },
        );
    }
}

async fn run_graph(
    app: &AppHandle,
    services: &GraphRunServices,
    store: &GraphStore,
    plan: GraphPlanRecord,
    run: GraphRunSummary,
    cancel_rx: watch::Receiver<bool>,
) -> Result<()> {
    let plan_id = plan.id.clone();
    let workspace_id = plan.workspace_id.clone();
    let definition: GraphDefinition =
        serde_json::from_str(&plan.definition_json).context("解析图定义失败")?;
    let workspace_root = resolve_workspace_root(&services.db, &workspace_id).await?;
    services
        .project_mcp_registry
        .ensure_recent(&workspace_root)
        .await
        .map_err(anyhow::Error::msg)
        .context("刷新 MCP 工具目录失败")?;
    let settings = tokio::task::spawn_blocking({
        let db = services.db.clone();
        move || db.get_settings_v2()
    })
    .await
    .context("读取设置任务失败")??;
    let tool_registry = Arc::new(ToolRegistry::default_tools(
        services.project_mcp_registry.clone(),
        services.ssh_manager.clone(),
    ));
    let catalog = build_harness_catalog(&workspace_root, &settings, &tool_registry).await;
    validate_graph(&definition, &catalog).map_err(anyhow::Error::msg)?;
    let layers = topological_layers(&definition).map_err(anyhow::Error::msg)?;
    let mut harnesses = HashMap::new();
    for node in &definition.nodes {
        harnesses.insert(
            node.id.clone(),
            resolve_node_harness(node, &settings, &tool_registry, &workspace_root)?,
        );
        store
            .save_node_run_async(&GraphNodeRunRecord::pending(&run.id, &plan_id, node))
            .await?;
    }
    store.update_plan_state_async(&plan_id, "{}").await?;
    emit_plan_updated(app, &plan_id, &workspace_id);
    emit_run_event(
        app,
        &plan_id,
        &run.id,
        &workspace_id,
        GraphRunEvent::RunStarted {
            title: definition.title.clone(),
            attempt_no: run.attempt_no,
            node_count: definition.nodes.len(),
        },
    );

    let provider = resolve_project_chat_provider(&services.agent_config, &settings);
    let session_title = services
        .db
        .get_session_title_async(&workspace_id)
        .await
        .unwrap_or_else(|_| "untitled".into());
    let user_requirement = services
        .db
        .get_latest_user_message_content_async(&workspace_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let base_tool_context = ToolContext {
        workspace_id: workspace_id.clone(),
        workspace: workspace_root.clone(),
        session_title,
        user_task: Some(user_requirement.clone()),
        ssh_review: settings
            .review
            .is_configured()
            .then_some(settings.review.clone()),
        exec_timeout_secs: 60,
        restrict_to_workspace: true,
        extra_allowed_dirs: dirs::home_dir()
            .map(|home| vec![home.join(".jkcodingagent")])
            .unwrap_or_default(),
        app_handle: Some(app.clone()),
        llm_provider: Some(provider),
        vision_model: String::new(),
        image_model_url: String::new(),
        image_model_api_key: String::new(),
        image_model: String::new(),
        image_edit_model: String::new(),
        sub_agent_tool_registry: None,
        current_sub_agent_id: None,
        current_sub_agent_name: None,
        current_tool_call_id: None,
        sub_agent_parent_tool_call_id: None,
        sub_agent_trace_events: None,
    };

    let mut state: Map<String, Value> = Map::new();
    let node_by_id = definition
        .nodes
        .iter()
        .map(|node| (node.id.trim(), node))
        .collect::<HashMap<_, _>>();
    let mut outputs = HashMap::new();
    let mut failed_nodes = Vec::new();
    let mut skipped_nodes = Vec::new();
    let mut blocked = HashSet::new();
    let mut cancelled = false;
    for layer in &layers {
        if *cancel_rx.borrow() {
            cancelled = true;
            cancel_pending_nodes(
                app,
                store,
                &plan_id,
                &run.id,
                &workspace_id,
                &definition.nodes,
            )
            .await;
            break;
        }
        let mut runnable = Vec::new();
        for node_id in layer {
            let node = node_by_id[node_id.as_str()];
            if node
                .depends_on
                .iter()
                .any(|dep| blocked.contains(dep.trim()))
            {
                mark_node_skipped(
                    app,
                    store,
                    &plan_id,
                    &run.id,
                    &workspace_id,
                    node,
                    "上游节点失败或被跳过",
                )
                .await;
                skipped_nodes.push(node.id.clone());
                blocked.insert(node.id.trim().to_string());
            } else {
                runnable.push(node)
            }
        }
        let mut iter = runnable.into_iter();
        let mut joins: JoinSet<NodeTaskResult> = JoinSet::new();
        loop {
            while joins.len() < MAX_PARALLEL_NODES {
                let Some(node) = iter.next() else { break };
                let input = assemble_node_input(&user_requirement, node, &outputs, &state);
                let harness = harnesses.remove(&node.id).context("节点 Harness 丢失")?;
                joins.spawn(run_node_task(NodeTaskContext {
                    exec: NodeExecContext {
                        app: app.clone(),
                        plan_id: plan_id.clone(),
                        run_id: run.id.clone(),
                        workspace_id: workspace_id.clone(),
                        workspace_root: workspace_root.clone(),
                        node: node.clone(),
                        input: input.clone(),
                        harness,
                        tool_registry: Arc::clone(&tool_registry),
                        tool_context: base_tool_context.clone(),
                        store: store.clone(),
                        cancel_rx: cancel_rx.clone(),
                    },
                    store: store.clone(),
                    input,
                }));
            }
            if joins.is_empty() {
                break;
            }
            let Some(joined) = joins.join_next().await else {
                break;
            };
            let mut result = match joined {
                Ok(result) => result,
                Err(error) => {
                    return Err(anyhow::anyhow!("节点任务异常：{error}"));
                }
            };
            match result.outcome {
                NodeExecOutcome::Succeeded {
                    output,
                    affected_files,
                    tool_call_count,
                    usage_json,
                } => {
                    result.record.affected_files = affected_files.clone();
                    result.record.tool_call_count = tool_call_count;
                    result.record.usage_json = usage_json;
                    let state_value =
                        truncate_for_display(&output, STATE_VALUE_MAX_CHARS, STATE_TRUNCATE_SUFFIX);
                    state.insert(
                        result.output_key.clone(),
                        Value::String(state_value.clone()),
                    );
                    finish_node_record(store, result.record, NODE_SUCCEEDED, Some(&output), None)
                        .await;
                    emit_run_event(
                        app,
                        &plan_id,
                        &run.id,
                        &workspace_id,
                        GraphRunEvent::NodeFinished {
                            node_id: result.node_id.clone(),
                            output: output.clone(),
                            duration_ms: result.duration_ms,
                            affected_files,
                        },
                    );
                    persist_and_emit_state(
                        app,
                        store,
                        &plan_id,
                        &run.id,
                        &workspace_id,
                        &result.node_id,
                        &result.output_key,
                        &state_value,
                        &state,
                    )
                    .await;
                    outputs.insert(result.node_id.trim().to_string(), output);
                }
                NodeExecOutcome::Failed(error) => {
                    finish_node_record(store, result.record, NODE_FAILED, None, Some(&error)).await;
                    emit_run_event(
                        app,
                        &plan_id,
                        &run.id,
                        &workspace_id,
                        GraphRunEvent::NodeFailed {
                            node_id: result.node_id.clone(),
                            error,
                            duration_ms: result.duration_ms,
                            affected_files: vec![],
                        },
                    );
                    failed_nodes.push(result.node_id.clone());
                    blocked.insert(result.node_id.trim().to_string());
                }
                NodeExecOutcome::Cancelled => {
                    finish_node_record(store, result.record, NODE_CANCELLED, None, Some("已取消"))
                        .await;
                    cancelled = true;
                }
            }
        }
        if *cancel_rx.borrow() {
            cancelled = true;
            cancel_pending_nodes(
                app,
                store,
                &plan_id,
                &run.id,
                &workspace_id,
                &definition.nodes,
            )
            .await;
            break;
        }
    }
    if cancelled {
        store.finish_run_async(&run.id, PLAN_CANCELLED).await?;
        store
            .update_plan_status_async(&plan_id, PLAN_CANCELLED)
            .await?;
        emit_plan_updated(app, &plan_id, &workspace_id);
        emit_run_event(
            app,
            &plan_id,
            &run.id,
            &workspace_id,
            GraphRunEvent::RunCancelled {},
        );
        return Ok(());
    }
    let status = if failed_nodes.is_empty() {
        PLAN_COMPLETED
    } else {
        PLAN_FAILED
    };
    store.finish_run_async(&run.id, status).await?;
    store.update_plan_status_async(&plan_id, status).await?;
    emit_plan_updated(app, &plan_id, &workspace_id);
    emit_run_event(
        app,
        &plan_id,
        &run.id,
        &workspace_id,
        GraphRunEvent::RunFinished {
            state: Value::Object(state),
            failed_nodes,
            skipped_nodes,
        },
    );
    Ok(())
}

async fn resolve_workspace_root(db: &DispatcherDb, workspace_id: &str) -> Result<PathBuf> {
    let project_id = db
        .get_session_project_id_async(workspace_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("找不到图计划所属会话：{workspace_id}"))?;
    let lookup = project_id.clone();
    let path = tokio::task::spawn_blocking(move || {
        crate::project::storage::load_projects()
            .ok()
            .and_then(|projects| {
                projects
                    .into_iter()
                    .find(|project| project.id == lookup)
                    .map(|project| project.path)
            })
    })
    .await
    .context("读取项目列表任务失败")?;
    path.map(PathBuf::from).ok_or_else(|| {
        anyhow::anyhow!("无法定位图计划所属项目路径（项目 {project_id} 可能已被删除）")
    })
}

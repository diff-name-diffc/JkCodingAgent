//! 图执行引擎。
//!
//! 拓扑分层调度：同层节点 `tokio::task::JoinSet` 并行（上限 3），跨层串行。
//! 节点输入装配 = 总体需求 + 角色 + 子任务 + 上游输出 + injectStateKeys 指定的
//! state 节选；节点完成后 `state[outputKey] = output`（截断 32k）并持久化。
//! 失败策略：节点失败 → 其下游（传递闭包）标记 skipped，其余分支继续。
//! 取消：命中 watch 标志后运行中的 CLI 子进程被 kill、子智能体在当前迭代结束后
//! 退出、未开始的节点标记 cancelled。
//!
//! 节点级生命周期在 `node_task`、节点输入装配在 `input`、节点执行器在 `node_exec`。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde_json::{Map, Value};
use tauri::{AppHandle, Emitter};
use tokio::sync::watch;
use tokio::task::JoinSet;

use super::input::assemble_node_input;
use super::node_exec::{NodeExecContext, NodeExecOutcome};
use super::node_task::{
    cancel_pending_nodes, finish_node_record, mark_node_skipped, persist_and_emit_state,
    run_node_task, NodeTaskContext, NodeTaskResult,
};
use super::store::GraphStore;
use super::types::{
    GraphDefinition, GraphNode, GraphNodeRunRecord, GraphPlanRecord, GraphPlanUpdatedPayload,
    GraphRunEvent, GraphRunEventPayload, NODE_CANCELLED, NODE_FAILED, NODE_SUCCEEDED,
    PLAN_CANCELLED, PLAN_COMPLETED, PLAN_FAILED, PLAN_RUNNING, STATE_VALUE_MAX_CHARS,
};
use super::validate::topological_layers;
use crate::agent::agents::project::resolve_project_chat_provider;
use crate::agent::config::DispatcherAgentConfig;
use crate::agent::db::DispatcherDb;
use crate::agent::sub_agent::SubAgentManager;
use crate::agent::tools::ToolRegistry;
use crate::project::mcp::ProjectMcpRegistry;
use crate::shared::truncate_for_display;
use crate::ssh_tool::SshSessionManager;

/// 同层并行上限。
const MAX_PARALLEL_NODES: usize = 3;
/// 节点输出写回 state 的截断后缀。
const STATE_TRUNCATE_SUFFIX: &str = "\n...[输出已截断]";

/// 图运行所需的长寿服务集合（由 DispatcherState 装配）。
pub(crate) struct GraphRunServices {
    pub db: DispatcherDb,
    pub agent_config: DispatcherAgentConfig,
    pub project_mcp_registry: ProjectMcpRegistry,
    pub ssh_manager: SshSessionManager,
    pub sub_agent_manager: Option<Arc<SubAgentManager>>,
}

// ─── 事件发射 ────────────────────────────────────────────────────────────────

pub(crate) fn emit_run_event(
    app: &AppHandle,
    plan_id: &str,
    workspace_id: &str,
    event: GraphRunEvent,
) {
    let payload = GraphRunEventPayload {
        plan_id: plan_id.to_string(),
        workspace_id: workspace_id.to_string(),
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        event,
    };
    let _ = app.emit("graph-run-event", payload);
}

pub(crate) fn emit_plan_updated(app: &AppHandle, plan_id: &str, workspace_id: &str) {
    let _ = app.emit(
        "graph-plan-updated",
        GraphPlanUpdatedPayload {
            plan_id: plan_id.to_string(),
            workspace_id: workspace_id.to_string(),
        },
    );
}

// ─── 入口 ────────────────────────────────────────────────────────────────────

/// 图运行入口（由 `graph_run_start` 命令 spawn）。内部错误统一收口为
/// plan=failed + runFailed 事件，绝不静默退出。
pub(crate) async fn execute_graph_run(
    app: AppHandle,
    services: GraphRunServices,
    plan_id: String,
    cancel_rx: watch::Receiver<bool>,
) {
    let store = GraphStore::new(&services.db);
    let plan = match store.get_plan_async(&plan_id).await {
        Ok(Some(plan)) => plan,
        Ok(None) => {
            eprintln!("[graph] 图计划不存在：{plan_id}");
            return;
        }
        Err(error) => {
            eprintln!("[graph] 读取图计划失败（{plan_id}）：{error:#}");
            return;
        }
    };
    let workspace_id = plan.workspace_id.clone();

    if let Err(error) = run_graph(&app, &services, &store, plan, cancel_rx).await {
        let message = format!("{error:#}");
        eprintln!("[graph] 图运行失败（{plan_id}）：{message}");
        if let Err(error) = store.update_plan_status_async(&plan_id, PLAN_FAILED).await {
            eprintln!("[graph] 更新图计划状态失败（{plan_id}）：{error:#}");
        }
        emit_plan_updated(&app, &plan_id, &workspace_id);
        emit_run_event(
            &app,
            &plan_id,
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
    cancel_rx: watch::Receiver<bool>,
) -> Result<()> {
    let plan_id = plan.id.clone();
    let workspace_id = plan.workspace_id.clone();
    let definition: GraphDefinition = serde_json::from_str(&plan.definition_json)
        .context("解析图定义失败（definition_json 损坏）")?;
    let layers = topological_layers(&definition).map_err(anyhow::Error::msg)?;

    // 重置节点运行记录并尽早置 running（环境装配失败也能留下正确的状态轨迹）。
    for node in &definition.nodes {
        store
            .save_node_run_async(&GraphNodeRunRecord::pending(&plan_id, node))
            .await?;
    }
    // 重跑时共享 state 从空开始（旧 state 一并清空）。
    store.update_plan_state_async(&plan_id, "{}").await?;
    store.update_plan_status_async(&plan_id, PLAN_RUNNING).await?;
    emit_plan_updated(app, &plan_id, &workspace_id);
    emit_run_event(
        app,
        &plan_id,
        &workspace_id,
        GraphRunEvent::RunStarted {
            title: definition.title.clone(),
            node_count: definition.nodes.len(),
        },
    );

    // ── 运行环境装配 ────────────────────────────────────────────────────────
    let workspace_root = resolve_workspace_root(&services.db, &workspace_id).await?;
    // MCP 工具定义依赖热缓存；刷新失败仅降级（MCP 工具不可用），不阻断图运行。
    if let Err(error) = services
        .project_mcp_registry
        .ensure_recent(&workspace_root)
        .await
    {
        eprintln!("[graph] 刷新项目 MCP 状态失败（降级继续）：{error}");
    }
    let settings = tokio::task::spawn_blocking({
        let db = services.db.clone();
        move || db.get_settings_v2()
    })
    .await
    .context("读取设置任务失败")??;
    let parent_provider = resolve_project_chat_provider(&services.agent_config, &settings);
    let tool_registry = Arc::new(ToolRegistry::default_tools(
        services.project_mcp_registry.clone(),
        services.ssh_manager.clone(),
    ));
    let session_title = services
        .db
        .get_session_title_async(&workspace_id)
        .await
        .unwrap_or_else(|_| "untitled".to_string());
    let user_requirement = services
        .db
        .get_latest_user_message_content_async(&workspace_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();

    // 共享 state：重跑时从空开始（启动时已把 state_json 重置为 {}）。
    let mut state: Map<String, Value> = Map::new();
    let node_by_id: HashMap<&str, &GraphNode> = definition
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();

    // ── 分层调度 ────────────────────────────────────────────────────────────
    let mut outputs: HashMap<String, String> = HashMap::new();
    let mut failed_nodes: Vec<String> = Vec::new();
    let mut skipped_nodes: Vec<String> = Vec::new();
    let mut blocked: HashSet<String> = HashSet::new(); // 失败/被跳过节点 id（用于下游 skipped 判定）
    let mut cancelled = false;
    // 节点启动前的 git 变更集快照（node_id → 前置快照），用于节点结束后差分出影响文件。
    let mut pre_run_snapshots: HashMap<String, HashSet<String>> = HashMap::new();

    for layer in &layers {
        if cancellation_requested(&cancel_rx) {
            cancelled = true;
            cancel_pending_nodes(app, store, &plan_id, &workspace_id, &definition.nodes).await;
            break;
        }

        // 上游（传递闭包）失败/被跳过的节点直接 skipped，其余分支继续。
        let mut runnable: Vec<&GraphNode> = Vec::new();
        for node_id in layer {
            let node = node_by_id[node_id.as_str()];
            if node.depends_on.iter().any(|dep| blocked.contains(dep)) {
                mark_node_skipped(
                    app,
                    store,
                    &plan_id,
                    &workspace_id,
                    node,
                    "上游节点失败或被跳过",
                )
                .await;
                skipped_nodes.push(node.id.clone());
                blocked.insert(node.id.clone());
            } else {
                runnable.push(node);
            }
        }

        // 同层 JoinSet 并行（上限 3）。依赖关系保证同层节点互不依赖，
        // 因此输入可在层首一次性装配。
        let mut iter = runnable.into_iter();
        let mut join_set: JoinSet<NodeTaskResult> = JoinSet::new();
        loop {
            while join_set.len() < MAX_PARALLEL_NODES {
                let Some(node) = iter.next() else { break };
                let input = assemble_node_input(&user_requirement, node, &outputs, &state);
                // spawn 前快照 git 变更集，节点结束后差分得到「节点影响文件」。
                let before = git_changed_paths(&workspace_root).await;
                pre_run_snapshots.insert(node.id.clone(), before);
                join_set.spawn(run_node_task(NodeTaskContext {
                    exec: NodeExecContext {
                        app: app.clone(),
                        plan_id: plan_id.clone(),
                        workspace_id: workspace_id.clone(),
                        workspace_root: workspace_root.clone(),
                        session_title: session_title.clone(),
                        user_requirement: user_requirement.clone(),
                        node: node.clone(),
                        input: input.clone(),
                        parent_provider: parent_provider.clone(),
                        tool_registry: Arc::clone(&tool_registry),
                        sub_agent_manager: services.sub_agent_manager.clone(),
                        cancel_rx: cancel_rx.clone(),
                    },
                    store: store.clone(),
                    input,
                }));
            }
            if join_set.is_empty() {
                break;
            }
            let Some(result) = join_set.join_next().await else {
                break;
            };
            let mut result = match result {
                Ok(result) => result,
                Err(join_error) => {
                    // catch_unwind 已兜住 panic，此处仅剩运行时级异常。
                    eprintln!("[graph] 节点任务异常终止：{join_error}");
                    continue;
                }
            };
            match result.outcome {
                NodeExecOutcome::Succeeded(output) => {
                    // 节点影响文件：git status 快照差分（后 - 前），排序保证输出稳定。
                    // 局限：基于快照差分——节点执行前已被弄脏的文件再被修改不会被捕获；
                    // 同层并行节点的文件变更可能互相归因（快照时刻无法区分归属）。
                    let before = pre_run_snapshots.remove(&result.node_id).unwrap_or_default();
                    let mut affected: Vec<String> = git_changed_paths(&workspace_root)
                        .await
                        .difference(&before)
                        .cloned()
                        .collect();
                    affected.sort();
                    result.record.affected_files = affected.clone();
                    let state_value =
                        truncate_for_display(&output, STATE_VALUE_MAX_CHARS, STATE_TRUNCATE_SUFFIX);
                    state.insert(result.output_key.clone(), Value::String(state_value.clone()));
                    finish_node_record(
                        store,
                        result.record,
                        NODE_SUCCEEDED,
                        Some(&output),
                        None,
                    )
                    .await;
                    emit_run_event(
                        app,
                        &plan_id,
                        &workspace_id,
                        GraphRunEvent::NodeFinished {
                            node_id: result.node_id.clone(),
                            output: output.clone(),
                            duration_ms: result.duration_ms,
                            affected_files: affected,
                        },
                    );
                    persist_and_emit_state(
                        app,
                        store,
                        &plan_id,
                        &workspace_id,
                        &result.node_id,
                        &result.output_key,
                        &state_value,
                        &state,
                    )
                    .await;
                    outputs.insert(result.node_id.clone(), output);
                }
                NodeExecOutcome::Failed(error) => {
                    // 失败节点同样差分影响文件（局限同 Succeeded 分支）。
                    let before = pre_run_snapshots.remove(&result.node_id).unwrap_or_default();
                    let mut affected: Vec<String> = git_changed_paths(&workspace_root)
                        .await
                        .difference(&before)
                        .cloned()
                        .collect();
                    affected.sort();
                    result.record.affected_files = affected.clone();
                    finish_node_record(store, result.record, NODE_FAILED, None, Some(&error))
                        .await;
                    emit_run_event(
                        app,
                        &plan_id,
                        &workspace_id,
                        GraphRunEvent::NodeFailed {
                            node_id: result.node_id.clone(),
                            error,
                            duration_ms: result.duration_ms,
                            affected_files: affected,
                        },
                    );
                    failed_nodes.push(result.node_id.clone());
                    blocked.insert(result.node_id.clone());
                }
                NodeExecOutcome::Cancelled => {
                    pre_run_snapshots.remove(&result.node_id);
                    finish_node_record(store, result.record, NODE_CANCELLED, None, Some("已取消"))
                        .await;
                    emit_run_event(
                        app,
                        &plan_id,
                        &workspace_id,
                        GraphRunEvent::NodeFailed {
                            node_id: result.node_id.clone(),
                            error: "节点已取消".to_string(),
                            duration_ms: result.duration_ms,
                            affected_files: Vec::new(),
                        },
                    );
                }
            }
        }

        // 层内未轮到启动的节点：由 cancel_pending_nodes 统一标记 cancelled。
        if cancellation_requested(&cancel_rx) {
            cancelled = true;
            cancel_pending_nodes(app, store, &plan_id, &workspace_id, &definition.nodes).await;
            break;
        }
    }

    // ── 收尾 ────────────────────────────────────────────────────────────────
    if cancelled {
        store
            .update_plan_status_async(&plan_id, PLAN_CANCELLED)
            .await?;
        emit_plan_updated(app, &plan_id, &workspace_id);
        emit_run_event(
            app,
            &plan_id,
            &workspace_id,
            GraphRunEvent::RunCancelled {},
        );
        return Ok(());
    }

    let final_status = if failed_nodes.is_empty() {
        PLAN_COMPLETED
    } else {
        PLAN_FAILED
    };
    store.update_plan_status_async(&plan_id, final_status).await?;
    emit_plan_updated(app, &plan_id, &workspace_id);
    emit_run_event(
        app,
        &plan_id,
        &workspace_id,
        GraphRunEvent::RunFinished {
            state: Value::Object(state),
            failed_nodes,
            skipped_nodes,
        },
    );
    Ok(())
}

// ─── 辅助 ────────────────────────────────────────────────────────────────────

fn cancellation_requested(cancel_rx: &watch::Receiver<bool>) -> bool {
    *cancel_rx.borrow()
}

/// 采集工作区当前的 git 变更路径集合（节点影响文件的快照来源）。
/// 非 git 仓库 / git 不可用 / 命令失败一律返回空集合——静默降级，不阻断图运行。
async fn git_changed_paths(root: &std::path::Path) -> HashSet<String> {
    let output = match tokio::process::Command::new("git")
        .args(["status", "--porcelain=v1", "--untracked-files=all"])
        .current_dir(root)
        .output()
        .await
    {
        Ok(output) if output.status.success() => output,
        _ => return HashSet::new(),
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            // porcelain v1 行格式：`XY<空格>路径`；rename 行为 `old -> new`，取新路径。
            let path = line.get(3..)?.trim();
            if path.is_empty() {
                return None;
            }
            let path = path.rsplit(" -> ").next().unwrap_or(path);
            Some(path.trim_matches('"').to_string())
        })
        .collect()
}

/// 会话 → 项目根路径：dispatcher_sessions.project_id → 项目列表中的 path。
async fn resolve_workspace_root(db: &DispatcherDb, workspace_id: &str) -> Result<PathBuf> {
    let project_id = db
        .get_session_project_id_async(workspace_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("找不到图计划所属会话：{workspace_id}"))?;
    let project_id_for_lookup = project_id.clone();
    let path = tokio::task::spawn_blocking(move || {
        crate::project::storage::load_projects()
            .map(|projects| {
                projects
                    .into_iter()
                    .find(|project| project.id == project_id_for_lookup)
                    .map(|project| project.path)
            })
            .unwrap_or(None)
    })
    .await
    .context("读取项目列表任务失败")?;
    path.map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("无法定位图计划所属项目路径（项目 {project_id} 可能已被删除）"))
}

//! PI 执行图 v3 运行器：ready-queue 依赖驱动调度。
//!
//! 方法论升级（相对 v2 层屏障调度）：
//! - 节点完成即解锁下游（scheduler::ReadyQueue 纯状态机驱动）；
//! - 节点失败先重试一次（输入注入失败原因），仍失败才阻断下游；
//! - resume 模式复用上次运行的成功节点（cached）与共享 state，实现断点续跑；
//! - 高危写检查点：设置开启时，就绪节点只剩 coding 节点即暂停全 run 等待恢复
//!   （暂停不阻塞已就绪的只读节点；任何 coding 节点不会在确认前启动）；
//! - 收尾由 verifier 产出验收结论、receipt 把执行回执写回会话消息（闭环）。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde_json::{Map, Value};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;

use super::harness::{build_harness_catalog, resolve_node_harness};
use super::input::{assemble_node_input, state_value_from_output};
use super::node_exec::{NodeExecContext, NodeExecOutcome};
use super::node_task::{
    cancel_pending_nodes, finish_node_record, mark_node_skipped, persist_and_emit_state,
    run_node_task, NodeTaskContext, NodeTaskResult,
};
use super::receipt;
use super::scheduler::{FinishKind, ReadyQueue, MAX_PARALLEL_NODES};
use super::store::GraphStore;
use super::types::{
    BaseToolGroup, GraphDefinition, GraphNodeRunRecord, GraphPlanRecord, GraphPlanUpdatedPayload,
    GraphRunEvent, GraphRunEventPayload, GraphRunSummary, NODE_CANCELLED, NODE_FAILED,
    NODE_SUCCEEDED, PLAN_CANCELLED, PLAN_COMPLETED, PLAN_FAILED, RUN_MODE_RESUME,
};
use super::validate::validate_graph;
use super::verifier;
use crate::agent::agents::project::resolve_project_chat_provider;
use crate::agent::config::DispatcherAgentConfig;
use crate::agent::db::DispatcherDb;
use crate::agent::state::GraphRunHandle;
use crate::agent::tools::{ToolContext, ToolRegistry};
use crate::project::mcp::ProjectMcpRegistry;
use crate::ssh_tool::SshSessionManager;

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
    mode: String,
    handle: GraphRunHandle,
) {
    let store = GraphStore::new(&services.db);
    let Some(plan) = store.get_plan_async(&plan_id).await.ok().flatten() else {
        eprintln!("[graph] 图计划不存在：{plan_id}");
        return;
    };
    let workspace_id = plan.workspace_id.clone();
    // resume：以最近一次运行（命令层已校验为 failed/cancelled）为续跑基线。
    let run = if mode == RUN_MODE_RESUME {
        let Some(latest) = store.get_latest_run_async(&plan_id).await.ok().flatten() else {
            eprintln!("[graph] 续跑失败：没有可续跑的历史运行（{plan_id}）");
            let _ = store.update_plan_status_async(&plan_id, PLAN_FAILED).await;
            emit_plan_updated(&app, &plan_id, &workspace_id);
            return;
        };
        store.create_resume_run_async(&plan_id, &latest.id).await
    } else {
        store.create_run_async(&plan_id).await
    };
    let run = match run {
        Ok(run) => run,
        Err(error) => {
            eprintln!("[graph] 创建运行失败：{error:#}");
            let _ = store.update_plan_status_async(&plan_id, PLAN_FAILED).await;
            emit_plan_updated(&app, &plan_id, &workspace_id);
            return;
        }
    };
    // create_run/create_resume_run 可能调整了 state_json（继承保留/续跑保留），重新装载。
    let Some(plan) = store.get_plan_async(&plan_id).await.ok().flatten() else {
        eprintln!("[graph] 图计划在运行创建后消失：{plan_id}");
        return;
    };
    let run_id = run.id.clone();
    if let Err(error) =
        run_graph(&app, &services, &store, plan, run, handle.cancel_rx, handle.resume_rx).await
    {
        let message = format!("{error:#}");
        eprintln!("[graph] 图运行失败（{plan_id}）：{message}");
        if let Err(error) = store.fail_interrupted_runs_async(Some(&plan_id)).await {
            eprintln!("[graph] 恢复中断运行状态失败（{plan_id}）：{error:#}");
        }
        emit_plan_updated(&app, &plan_id, &workspace_id);
        emit_run_event(
            &app,
            &plan_id,
            &run_id,
            &workspace_id,
            GraphRunEvent::RunFailed { error: message },
        );
    }
}

async fn wait_for_cancel(cancel_rx: &mut watch::Receiver<bool>) {
    while !*cancel_rx.borrow() {
        if cancel_rx.changed().await.is_err() {
            // 发送端被丢弃：按未取消处理，交给节点自身的超时兜底。
            std::future::pending::<()>().await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_graph(
    app: &AppHandle,
    services: &GraphRunServices,
    store: &GraphStore,
    plan: GraphPlanRecord,
    run: GraphRunSummary,
    mut cancel_rx: watch::Receiver<bool>,
    mut resume_rx: mpsc::UnboundedReceiver<()>,
) -> Result<()> {
    let plan_id = plan.id.clone();
    let workspace_id = plan.workspace_id.clone();
    let mut definition: GraphDefinition =
        serde_json::from_str(&plan.definition_json).context("解析图定义失败")?;
    // 存量 v2 计划一次性升级到 v3（新字段均有 serde default），否则下方
    // validate_graph 会以版本号拒绝历史计划重跑/续跑。
    definition.upgrade_legacy();
    // 存量定义可能残留带空白的节点 id：入口统一规整，保证调度器、持久化、
    // 事件与 DB 记录全程使用同一套 id（ReadyQueue/validate 均以 trim id 为准）。
    definition.normalize_ids();
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
    // 种子键：plan 当前 state 里的键（普通图为空；修复图/续跑携带继承或既有键）。
    // 解析失败必须显式报错而非静默退化为空集：空种子键会让 validate 把依赖
    // 继承键的节点误报为 missing-input（错误指向图定义而非真正的 state 损坏），
    // 且损坏的 state_json 会在下方被再次解析成空 Map，让续跑丢失既有共享 state。
    let plan_state: Map<String, Value> =
        serde_json::from_str(&plan.state_json).map_err(|error| {
            anyhow::anyhow!(
                "图计划共享 state 已损坏（plan_id={}，JSON 解析失败：{error}），无法运行",
                plan.id
            )
        })?;
    let seeded_keys = plan_state.keys().cloned().collect::<HashSet<String>>();
    validate_graph(&definition, &catalog, &seeded_keys).map_err(anyhow::Error::msg)?;

    // resume 时 DB 已含复制的 cached succeeded 行：只对缺失节点写 pending。
    // key 统一 trim：兼容规整前落库的历史节点记录（node_id 可能带空白）。
    let existing_runs = store.list_node_runs_async(&run.id).await?;
    let initial_status = existing_runs
        .iter()
        .map(|record| (record.node_id.trim().to_string(), record.status.clone()))
        .collect::<HashMap<_, _>>();
    for node in &definition.nodes {
        if !initial_status.contains_key(node.id.trim()) {
            store
                .save_node_run_async(&GraphNodeRunRecord::pending(&run.id, &plan_id, node))
                .await?;
        }
    }

    let node_by_id = definition
        .nodes
        .iter()
        .map(|node| (node.id.trim().to_string(), node.clone()))
        .collect::<HashMap<_, _>>();
    let mut harnesses = HashMap::new();
    for node in &definition.nodes {
        harnesses.insert(
            node.id.trim().to_string(),
            resolve_node_harness(node, &settings, &tool_registry, &workspace_root)?,
        );
    }

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
    // v3：需求以提交时快照为准；快照为空时兜底取最新消息（防御旧数据）。
    let mut user_requirement = plan.requirement.trim().to_string();
    if user_requirement.is_empty() {
        user_requirement = services
            .db
            .get_latest_user_message_content_async(&workspace_id)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
    }
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

    // 初始共享 state 与上游输出：resume 复用 plan 现有 state 与 cached 节点产出。
    let mut state = plan_state;
    let mut outputs: HashMap<String, String> = existing_runs
        .iter()
        .filter(|record| record.status == NODE_SUCCEEDED)
        .map(|record| (record.node_id.trim().to_string(), record.output_text.clone()))
        .collect();

    let mut queue = ReadyQueue::new(&definition, &initial_status);
    // 防御性兜底：初始状态含 failed/cancelled 节点时级联 skip 其下游。
    // 注意当前数据流不会触发该分支——resume 的 create_resume_run 仅复制
    // status=succeeded 的行，其余节点一律由上方写 pending 重跑（基线失败
    // 节点因此会被自动重跑）；调用保留是给未来可能引入终态初始状态的
    // 调度路径兜底（ReadyQueue::new 支持该形态并有单测覆盖）。
    for skipped_id in queue.cascade_initial_terminal() {
        if let Some(node) = node_by_id.get(&skipped_id) {
            mark_node_skipped(
                app,
                store,
                &plan_id,
                &run.id,
                &workspace_id,
                node,
                "上游节点失败",
            )
            .await;
        }
    }
    let mut joins: JoinSet<NodeTaskResult> = JoinSet::new();
    let mut cancelled = false;
    let mut checkpoint_passed = false;
    let mut last_errors: HashMap<String, String> = HashMap::new();
    // 节点任务异常（panic）时停止派发新节点，先排空在途任务再整体收尾。
    let mut task_error: Option<anyhow::Error> = None;

    'main: loop {
        // 补满并发池。
        while task_error.is_none() && !cancelled && joins.len() < MAX_PARALLEL_NODES {
            let ready = queue.ready_nodes();
            // 高危写检查点通过前，优先派发就绪的非 coding 节点（如独立分支的
            // 调研节点），不让暂停阻塞无害的只读工作；剩余就绪节点全是 coding
            // 时才落入下方暂停分支。承诺不变：coding 节点不会在确认前启动。
            let node_id = if !checkpoint_passed && settings.graph.pause_before_write {
                ready
                    .iter()
                    .find(|id| {
                        node_by_id
                            .get(*id)
                            .map(|n| n.base_tool_group != BaseToolGroup::Coding)
                            .unwrap_or(false)
                    })
                    .or_else(|| ready.first())
                    .cloned()
            } else {
                ready.first().cloned()
            };
            let Some(node_id) = node_id else {
                break;
            };
            let node = node_by_id
                .get(&node_id)
                .context("调度器返回了未知节点")?
                .clone();

            // 高危写检查点：每个 run 只拦一次，就绪节点只剩 coding 时暂停等待恢复。
            if !checkpoint_passed
                && settings.graph.pause_before_write
                && node.base_tool_group == BaseToolGroup::Coding
            {
                emit_run_event(
                    app,
                    &plan_id,
                    &run.id,
                    &workspace_id,
                    GraphRunEvent::RunPaused {
                        node_id: node_id.clone(),
                    },
                );
                emit_plan_updated(app, &plan_id, &workspace_id);
                // 取消优先于恢复：biased 按分支顺序检查，cancel 放前面。
                tokio::select! {
                    biased;
                    _ = wait_for_cancel(&mut cancel_rx) => {
                        cancelled = true;
                    }
                    _ = resume_rx.recv() => {}
                }
                if cancelled {
                    // 不能直接 break：JoinSet 中可能已有检查点前并发启动的在途
                    // 节点（其记录已被 run_node_task 置为 running），直接跳出会
                    // 让它们在 JoinSet drop 时被 abort，记录永久停留在 running，
                    // 而后续 cancel_pending_nodes 只处理 pending 记录。与
                    // NodeExecOutcome::Cancelled 分支一致：回到主循环排空在途任务，
                    // 它们会经 cancel_rx 自行结算为 cancelled。
                    continue 'main;
                }
                checkpoint_passed = true;
                emit_run_event(
                    app,
                    &plan_id,
                    &run.id,
                    &workspace_id,
                    GraphRunEvent::RunResumed {},
                );
                emit_plan_updated(app, &plan_id, &workspace_id);
            }

            queue.claim(&node_id);
            let retry_count = queue.retry_count(&node_id);
            let input = assemble_node_input(
                &user_requirement,
                &node,
                &node_by_id,
                &outputs,
                &state,
                last_errors.get(&node_id).map(String::as_str),
            );
            let harness = harnesses
                .get(&node_id)
                .cloned()
                .context("节点 Harness 丢失")?;
            joins.spawn(run_node_task(NodeTaskContext {
                exec: NodeExecContext {
                    app: app.clone(),
                    plan_id: plan_id.clone(),
                    run_id: run.id.clone(),
                    workspace_id: workspace_id.clone(),
                    workspace_root: workspace_root.clone(),
                    node,
                    input: input.clone(),
                    harness,
                    tool_registry: Arc::clone(&tool_registry),
                    tool_context: base_tool_context.clone(),
                    store: store.clone(),
                    cancel_rx: cancel_rx.clone(),
                },
                store: store.clone(),
                input,
                retry_count,
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
                // 节点任务 panic（execute_node 内部已有 catch_unwind，此处兜底
                // 其外围的 panic）。不能直接 return：JoinSet drop 会 abort 其余
                // 在途任务——它们的记录将停留在 running，且已发出的 NodeStarted
                // 没有对应终态事件。停止派发、排空在途任务让其正常结算（产出
                // 各自的结果与事件），再整体返回错误；panic 节点停留在 running
                // 的记录由 fail_interrupted_runs 兜底置为 failed。
                eprintln!(
                    "[graph] 节点任务异常（{plan_id}）：{error}；停止派发，等待在途任务结算"
                );
                task_error = Some(anyhow::anyhow!("节点任务异常：{error}"));
                continue 'main;
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
                // 共享 state 承载「节点间流转的结论」：写回产出摘要（≤4k）而非全文，
                // 全文保留在 node_runs.output_text；确需完整产出的下游通过
                // dependsOn + exportPolicy=full 获取，而非 injectStateKeys。
                let state_value = state_value_from_output(&output);
                state.insert(result.output_key.clone(), Value::String(state_value.clone()));
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
                last_errors.remove(&result.node_id);
                queue.on_finished(&result.node_id, FinishKind::Succeeded);
            }
            NodeExecOutcome::Failed(error) => {
                // record_retry 返回 false 表示节点已被其他路径结算（状态守卫
                // 拒绝），按最终失败处理，不得把终态复活回 Pending 重试。
                if queue.retryable(&result.node_id) && queue.record_retry(&result.node_id) {
                    // 重试一次：记录本次失败，节点回到就绪队列，输入注入失败原因。
                    last_errors.insert(result.node_id.clone(), error.clone());
                    finish_node_record(
                        store,
                        result.record,
                        NODE_FAILED,
                        None,
                        Some(&format!("{error}（将自动重试一次）")),
                    )
                    .await;
                    emit_run_event(
                        app,
                        &plan_id,
                        &run.id,
                        &workspace_id,
                        GraphRunEvent::NodeFailed {
                            node_id: result.node_id.clone(),
                            error: format!("{error}（将自动重试一次）"),
                            duration_ms: result.duration_ms,
                            affected_files: vec![],
                        },
                    );
                } else {
                    let newly_skipped = queue.on_finished(&result.node_id, FinishKind::FailedFinal);
                    finish_node_record(store, result.record, NODE_FAILED, None, Some(&error))
                        .await;
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
                    // 传递性跳过的下游：落库并发事件。
                    for skipped_id in newly_skipped {
                        if let Some(node) = node_by_id.get(&skipped_id) {
                            mark_node_skipped(
                                app,
                                store,
                                &plan_id,
                                &run.id,
                                &workspace_id,
                                node,
                                "上游节点失败",
                            )
                            .await;
                        }
                    }
                }
            }
            NodeExecOutcome::Cancelled => {
                finish_node_record(store, result.record, NODE_CANCELLED, None, Some("已取消"))
                    .await;
                cancelled = true;
            }
        }
    }

    // 节点任务异常：在途任务已排空结算，整体按失败收尾（交回
    // execute_graph_run 置失败态、广播事件并兜底中断运行记录）。
    if let Some(error) = task_error {
        return Err(error);
    }

    if cancelled {
        cancel_pending_nodes(
            app,
            store,
            &plan_id,
            &run.id,
            &workspace_id,
            &definition.nodes,
        )
        .await;
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

    // 防御：理论上循环退出时不应有未结算节点（失败已传递 skip）；若有则兜底跳过。
    if queue.has_unsettled() {
        for remaining in queue.cancel_remaining() {
            if let Some(node) = node_by_id.get(&remaining) {
                mark_node_skipped(
                    app,
                    store,
                    &plan_id,
                    &run.id,
                    &workspace_id,
                    node,
                    "依赖未满足且未被跳过",
                )
                .await;
            }
        }
    }

    // 闭环收尾：验收 → 回执 → 终态落库。
    let node_runs = store.list_node_runs_async(&run.id).await?;
    let verdict = verifier::verify_run(
        &services.agent_config,
        &settings,
        &user_requirement,
        &definition,
        &state,
        &node_runs,
    )
    .await;
    if let Err(error) = store
        .update_run_verdict_async(&run.id, &verdict.status, &verdict.reason)
        .await
    {
        eprintln!("[graph] 写入验收结论失败（{plan_id}）：{error:#}");
    }

    let failed_nodes = queue.failed_nodes();
    let skipped_nodes = queue.skipped_nodes();
    let status = if failed_nodes.is_empty() {
        PLAN_COMPLETED
    } else {
        PLAN_FAILED
    };
    store.finish_run_async(&run.id, status).await?;
    store.update_plan_status_async(&plan_id, status).await?;
    emit_plan_updated(app, &plan_id, &workspace_id);

    receipt::deliver_receipt(
        app,
        &services.db,
        &workspace_id,
        &plan,
        &run,
        &node_runs,
        &state,
        &verdict,
    )
    .await;

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

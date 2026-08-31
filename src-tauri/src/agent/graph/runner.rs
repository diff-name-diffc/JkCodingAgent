//! PI 执行图 v3 运行器：ready-queue 依赖驱动调度。
//!
//! 方法论升级（相对 v2 层屏障调度）：
//! - 节点完成即解锁下游（scheduler::ReadyQueue 纯状态机驱动）；
//! - 节点失败先重试一次（输入注入失败原因），仍失败才阻断下游；
//! - resume 模式复用上次运行的成功节点（cached）与共享 state，实现断点续跑；
//! - 高危写检查点：设置开启时，就绪节点只剩「可能写盘」的节点即暂停全 run
//!   等待恢复（判定含 coding 工具组、可写特殊工具与 expectedFiles，见
//!   node_may_write；暂停不阻塞已就绪的只读节点；写盘节点不会在确认前启动）；
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
    BaseToolGroup, GraphDefinition, GraphHarnessCatalog, GraphNode, GraphNodeRunRecord,
    GraphPlanRecord, GraphPlanUpdatedPayload, GraphRunEvent, GraphRunEventPayload, GraphRunSummary,
    NODE_CANCELLED, NODE_FAILED, NODE_SUCCEEDED, PLAN_CANCELLED, PLAN_COMPLETED, PLAN_FAILED,
    RUN_MODE_RESUME,
};
use super::validate::validate_graph;
use super::verifier;
use crate::agent::agents::project::{resolve_project_chat_provider, resolve_vision_provider};
use crate::agent::config::DispatcherAgentConfig;
use crate::agent::db::DispatcherDb;
use crate::agent::state::GraphRunHandle;
use crate::agent::tools::{ToolContext, ToolRegistry};
use crate::mcp::{McpRegistry, McpScope};
use crate::ssh_tool::SshSessionManager;

static EVENT_SEQUENCE: AtomicI64 = AtomicI64::new(0);

pub(crate) struct GraphRunServices {
    pub db: DispatcherDb,
    pub agent_config: DispatcherAgentConfig,
    pub mcp_registry: McpRegistry,
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
    if let Err(error) = run_graph(
        &app,
        &services,
        &store,
        plan,
        run,
        handle.cancel_rx,
        handle.resume_rx,
    )
    .await
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
            // 发送端被丢弃：按取消处理（fail-closed）。永久 pending 会让
            // 高危写检查点无法了结——配合 resume 通道被丢弃（recv 返回 None）
            // 的分支，旧实现会滑向「未经确认静默继续」。
            return;
        }
    }
}

/// 高危写检查点的「节点可能写盘」判定：写能力来自三处合集的并集——
/// sidecar 内置工具组（coding 组含 write/edit/bash）、specialTools
/// （按目录条目的 readonly 标记；目录缺失的工具 fail-closed 视为可写）、
/// 以及 expectedFiles 声明。仅看 base_tool_group 会漏掉「误标 read_only
/// 但携带可写扩展工具」的节点，使其在检查点确认前就被派发执行写操作。
fn node_may_write(node: &GraphNode, catalog: &GraphHarnessCatalog) -> bool {
    if node.base_tool_group == BaseToolGroup::Coding || !node.expected_files.is_empty() {
        return true;
    }
    node.special_tools.iter().any(|tool_ref| {
        catalog
            .tools
            .iter()
            .find(|entry| entry.source == tool_ref.source && entry.name == tool_ref.name)
            .map(|entry| !entry.readonly)
            .unwrap_or(true)
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_graph(
    app: &AppHandle,
    services: &GraphRunServices,
    store: &GraphStore,
    plan: GraphPlanRecord,
    run: GraphRunSummary,
    mut cancel_rx: watch::Receiver<bool>,
    mut resume_rx: mpsc::Receiver<()>,
) -> Result<()> {
    let plan_id = plan.id.clone();
    let workspace_id = plan.workspace_id.clone();
    let mut definition: GraphDefinition =
        serde_json::from_str(&plan.definition_json).context("解析图定义失败")?;
    // 落库的定义理论上都已经过入口规整（见 normalize_ids 文档）；这里再规整
    // 一次作为加载边界兜底，保证调度器、持久化、事件与 DB 记录全程使用同一套 id。
    definition.normalize_ids();
    let workspace_root = resolve_workspace_root(&services.db, &workspace_id).await?;
    // resolve_workspace_root 已 canonicalize 且失败即错，直接构造项目作用域：
    // 图执行全程使用同一份合并配置快照。
    let mcp_scope = McpScope::Project(workspace_root.clone());
    services
        .mcp_registry
        .ensure_recent(&mcp_scope)
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
        services.mcp_registry.clone(),
        services.ssh_manager.clone(),
    ));
    let catalog =
        build_harness_catalog(&workspace_root, &mcp_scope, &settings, &tool_registry).await;
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
    let existing_runs = store.list_node_runs_async(&run.id).await?;
    let initial_status = existing_runs
        .iter()
        .map(|record| (record.node_id.clone(), record.status.clone()))
        .collect::<HashMap<_, _>>();
    for node in &definition.nodes {
        if !initial_status.contains_key(&node.id) {
            store
                .save_node_run_async(&GraphNodeRunRecord::pending(&run.id, &plan_id, node))
                .await?;
        }
    }

    let node_by_id = definition
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node.clone()))
        .collect::<HashMap<_, _>>();
    let mut harnesses = HashMap::new();
    for node in &definition.nodes {
        harnesses.insert(
            node.id.clone(),
            resolve_node_harness(node, &settings, &tool_registry, &mcp_scope)?,
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
    // 视觉用途凭据（可能独立于聊天网关）：供图节点运行期工具（如
    // analyze_image）直接调用视觉模型；未配置时为 None，工具报「视觉模型未配置」。
    let vision_provider = resolve_vision_provider(
        &settings.shared.vision_model_configs,
        &provider,
        services.agent_config.max_tokens,
        services.agent_config.temperature,
    );
    let vision_model = vision_provider
        .as_ref()
        .map(|p| p.model().to_string())
        .unwrap_or_default();
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
    let image_credentials = settings.shared.image_model_credentials();
    let base_tool_context = ToolContext {
        workspace_id: workspace_id.clone(),
        workspace: workspace_root.clone(),
        mcp_scope,
        session_title,
        user_task: Some(user_requirement.clone()),
        ssh_review: settings
            .review
            .is_configured()
            .then_some(settings.review.clone()),
        // 图片生成/编辑工具凭据（generate_image / edit_image 节点可用）。
        image_model_url: image_credentials.url.clone(),
        image_model_api_key: image_credentials.api_key.clone(),
        image_model: image_credentials.model.clone(),
        image_edit_model: image_credentials.edit_model.clone(),
        exec_timeout_secs: 60,
        restrict_to_workspace: true,
        // PI 文本资源由宿主加载；图节点的 read/exec 能力严格限定在项目工作区。
        extra_allowed_dirs: Vec::new(),
        app_handle: Some(app.clone()),
        llm_provider: Some(provider),
        vision_model,
        vision_provider,
        sub_agent_tool_registry: None,
        current_sub_agent_id: None,
        current_sub_agent_name: None,
        current_tool_call_id: None,
        current_tool_spec_hash: None,
        cancel_rx: None,
        sub_agent_parent_tool_call_id: None,
        sub_agent_trace_events: None,
    };
    // 初始共享 state 与上游输出：resume 复用 plan 现有 state 与 cached 节点产出。
    let mut state = plan_state;
    let mut outputs: HashMap<String, String> = existing_runs
        .iter()
        .filter(|record| record.status == NODE_SUCCEEDED)
        .map(|record| (record.node_id.clone(), record.output_text.clone()))
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
            // 高危写检查点通过前，优先派发就绪的「不可能写盘」节点（如独立分支
            // 的调研节点），不让暂停阻塞无害的只读工作；剩余就绪节点全都可能
            // 写盘时才落入下方暂停分支。承诺不变：写盘节点不会在确认前启动。
            let node_id = if !checkpoint_passed && settings.graph.pause_before_write {
                ready
                    .iter()
                    .find(|id| {
                        node_by_id
                            .get(*id)
                            .map(|n| !node_may_write(n, &catalog))
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
            let node = match node_by_id.get(&node_id) {
                Some(node) => node.clone(),
                // 不能直接 `?` 返回：JoinSet drop 会 abort 在途任务，节点记录
                // 停留 running。统一走停止派发→排空在途→整体返回的错误路径。
                None => {
                    task_error = Some(anyhow::anyhow!("调度器返回了未知节点：{node_id}"));
                    break;
                }
            };

            // 高危写检查点：每个 run 只拦一次，就绪节点只剩可能写盘的节点时
            // 暂停等待恢复。
            if !checkpoint_passed
                && settings.graph.pause_before_write
                && node_may_write(&node, &catalog)
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
                    signal = resume_rx.recv() => {
                        if signal.is_none() {
                            // 控制通道被丢弃：不能当作「确认恢复」。高危写
                            // 检查点未确认前必须按取消处理（fail-closed），
                            // 否则会违背「coding 节点不在确认前启动」的承诺。
                            cancelled = true;
                        }
                    }
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
            let harness = match harnesses.get(&node_id).cloned() {
                Some(harness) => harness,
                // 同「未知节点」分支：不能 `?` 直接返回，统一走停止派发→
                // 排空在途→整体返回的错误路径。
                None => {
                    task_error = Some(anyhow::anyhow!("节点 Harness 丢失：{node_id}"));
                    break;
                }
            };
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
                eprintln!("[graph] 节点任务异常（{plan_id}）：{error}；停止派发，等待在途任务结算");
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
                state.insert(
                    result.output_key.clone(),
                    Value::String(state_value.clone()),
                );
                finish_node_record(store, result.record, NODE_SUCCEEDED, Some(&output), None).await;
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
                if let Err(error) = persist_and_emit_state(
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
                .await
                {
                    // fail-closed：共享 state 落库失败则停止派发，排空在途任务后
                    // 整体按失败收尾（续跑可恢复），不得带漂移状态继续执行。
                    task_error = Some(error.context("持久化图共享状态失败"));
                    continue 'main;
                }
                outputs.insert(result.node_id.clone(), output);
                last_errors.remove(&result.node_id);
                queue.on_finished(&result.node_id, FinishKind::Succeeded);
            }
            NodeExecOutcome::Failed { error, usage_json } => {
                // 失败结算同样落库已发生的 usage：LLM 调用已消耗 token 后节点
                // 失败（含重试前的首次尝试），用量不得在记录层丢失。
                result.record.usage_json = usage_json;
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
        // fail-closed：节点记录读取失败时不执行取消落库（不得把未确认节点
        // 的记录当作空集全量覆盖），错误向上传播由整体失败路径收尾。
        cancel_pending_nodes(
            app,
            store,
            &plan_id,
            &run.id,
            &workspace_id,
            &definition.nodes,
        )
        .await?;
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
        receipt::ReceiptData {
            plan: &plan,
            run: &run,
            node_runs: &node_runs,
            state: &state,
            verdict: &verdict,
        },
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
    let db_for_lookup = db.clone();
    let path = tokio::task::spawn_blocking(move || {
        db_for_lookup
            .find_project(&lookup)
            .ok()
            .flatten()
            .map(|project| project.path)
    })
    .await
    .context("读取项目列表任务失败")?;
    let path = path.map(PathBuf::from).ok_or_else(|| {
        anyhow::anyhow!("无法定位图计划所属项目路径（项目 {project_id} 可能已被删除）")
    })?;
    // 规范化工作区根（解析符号链接与 ../）：后续 sidecar 消息、受影响文件
    // 校验都以此为基准，目录不存在时 fail-closed。
    let root = tokio::task::spawn_blocking(move || path.canonicalize())
        .await
        .context("规范化工作区路径任务失败")?
        .with_context(|| format!("工作区路径不存在或无法访问：{project_id}"))?;
    Ok(root)
}

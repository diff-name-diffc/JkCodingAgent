//! 工作流 v4 运行器：ready-queue 依赖驱动调度。
//!
//! 方法论（相对 v2 层屏障调度）：
//! - 节点完成即解锁下游（scheduler::ReadyQueue 纯状态机驱动）；
//! - 节点失败先重试一次（输入注入失败原因），仍失败才阻断下游；
//! - resume 模式复用上次运行的成功节点（cached）与共享 state，实现断点续跑；
//! - 高危写检查点：设置开启时，就绪节点只剩「可能写盘」的节点即暂停全 run
//!   等待恢复（判定含 coding 工具组与 expectedFiles，见 node_may_write；
//!   暂停不阻塞已就绪的只读节点；写盘节点不会在确认前启动）；
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

use super::acp_exec::permission_review::{
    build_workflow_digest, PermissionReviewMaterials, WorkflowReviewShared,
};
use super::acp_exec::{NodeExecContext, NodeExecOutcome};
use super::harness::{build_harness_catalog, resolve_node_harness};
use super::input::{assemble_node_input, state_value_from_output};
use super::node_task::{
    cancel_pending_nodes, finish_node_record, mark_node_skipped, persist_and_emit_state,
    run_node_task, NodeTaskContext, NodeTaskResult,
};
use super::receipt;
use super::scheduler::{FinishKind, ReadyQueue, MAX_PARALLEL_NODES};
use super::store::WorkflowStore;
use super::types::{
    BaseToolGroup, WorkflowDefinition, WorkflowNode, WorkflowNodeRunRecord, WorkflowPlanRecord,
    WorkflowPlanUpdatedPayload, WorkflowRunEvent, WorkflowRunEventPayload, WorkflowRunResult,
    WorkflowRunSummary, NODE_CANCELLED, NODE_FAILED, NODE_SUCCEEDED, PLAN_CANCELLED,
    PLAN_COMPLETED, PLAN_FAILED, RESULT_KIND_EDIT, RESULT_KIND_NONE, RESULT_KIND_REVIEW,
    RUN_MODE_RESUME,
};
use super::validate::validate_workflow;
use super::verifier;
use crate::agent::config::DispatcherAgentConfig;
use crate::agent::db::DispatcherDb;
use crate::agent::state::WorkflowRunHandle;

static EVENT_SEQUENCE: AtomicI64 = AtomicI64::new(0);

pub(crate) struct WorkflowRunServices {
    pub db: DispatcherDb,
    pub agent_config: DispatcherAgentConfig,
}

pub(crate) fn emit_run_event(
    app: &AppHandle,
    plan_id: &str,
    run_id: &str,
    workspace_id: &str,
    event: WorkflowRunEvent,
) {
    let payload = WorkflowRunEventPayload {
        plan_id: plan_id.into(),
        run_id: run_id.into(),
        workspace_id: workspace_id.into(),
        sequence: EVENT_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1,
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        event,
    };
    let _ = app.emit("workflow-run-event", payload);
}
pub(crate) fn emit_plan_updated(app: &AppHandle, plan_id: &str, workspace_id: &str) {
    let _ = app.emit(
        "workflow-plan-updated",
        WorkflowPlanUpdatedPayload {
            plan_id: plan_id.into(),
            workspace_id: workspace_id.into(),
        },
    );
}

pub(crate) async fn execute_workflow_run(
    app: AppHandle,
    services: WorkflowRunServices,
    plan_id: String,
    mode: String,
    handle: WorkflowRunHandle,
) {
    let store = WorkflowStore::new(&services.db);
    let Some(plan) = store.get_plan_async(&plan_id).await.ok().flatten() else {
        eprintln!("[workflow] 工作流计划不存在：{plan_id}");
        return;
    };
    let workspace_id = plan.workspace_id.clone();
    // resume：以最近一次运行（命令层已校验为 failed/cancelled）为续跑基线。
    let run = if mode == RUN_MODE_RESUME {
        let Some(latest) = store.get_latest_run_async(&plan_id).await.ok().flatten() else {
            eprintln!("[workflow] 续跑失败：没有可续跑的历史运行（{plan_id}）");
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
            eprintln!("[workflow] 创建运行失败：{error:#}");
            let _ = store.update_plan_status_async(&plan_id, PLAN_FAILED).await;
            emit_plan_updated(&app, &plan_id, &workspace_id);
            return;
        }
    };
    // create_run/create_resume_run 可能调整了 state_json（继承保留/续跑保留），重新装载。
    let Some(plan) = store.get_plan_async(&plan_id).await.ok().flatten() else {
        eprintln!("[workflow] 工作流计划在运行创建后消失：{plan_id}");
        return;
    };
    let run_id = run.id.clone();
    if let Err(error) = run_workflow(
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
        eprintln!("[workflow] 工作流运行失败（{plan_id}）：{message}");
        if let Err(error) = store.fail_interrupted_runs_async(Some(&plan_id)).await {
            eprintln!("[workflow] 恢复中断运行状态失败（{plan_id}）：{error:#}");
        }
        emit_plan_updated(&app, &plan_id, &workspace_id);
        emit_run_event(
            &app,
            &plan_id,
            &run_id,
            &workspace_id,
            WorkflowRunEvent::RunFailed { error: message },
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

/// 高危写检查点的「节点可能写盘」判定：coding 工具组（含 write/edit/bash）
/// 或 expectedFiles 声明任一即视为可写。
fn node_may_write(node: &WorkflowNode) -> bool {
    node.base_tool_group == BaseToolGroup::Coding || !node.expected_files.is_empty()
}

#[allow(clippy::too_many_arguments)]
async fn run_workflow(
    app: &AppHandle,
    services: &WorkflowRunServices,
    store: &WorkflowStore,
    plan: WorkflowPlanRecord,
    run: WorkflowRunSummary,
    mut cancel_rx: watch::Receiver<bool>,
    mut resume_rx: mpsc::Receiver<()>,
) -> Result<()> {
    let plan_id = plan.id.clone();
    let workspace_id = plan.workspace_id.clone();
    let mut definition: WorkflowDefinition =
        serde_json::from_str(&plan.definition_json).context("解析工作流定义失败")?;
    // 落库的定义理论上都已经过入口规整（见 normalize_ids 文档）；这里再规整
    // 一次作为加载边界兜底，保证调度器、持久化、事件与 DB 记录全程使用同一套 id。
    definition.normalize_ids();
    let workspace_root = resolve_workspace_root(&services.db, &workspace_id).await?;
    let canonical_root = tokio::task::spawn_blocking({
        let root = workspace_root.clone();
        move || root.canonicalize()
    })
    .await
    .context("规范化工作流工作区任务失败")??;
    // ACP 内部工具不可逐次声明资源，整个工作流运行持有工作区写租约。租约按路径嵌套判定
    // 冲突：只挡本工作区（同一或嵌套工作区）的文件/工作区域声明，跨项目会话不受影响；
    // 会话内文件工具因此排队超时时，超时文案会点名占用方（见 resources.rs 的
    // `AcquireError::timeout_message`）。
    use crate::agent::rig_ext::r#loop::resources::{Claim, Resource, ResourceArbiter};
    let _workspace_lease = ResourceArbiter::shared()
        .acquire(
            vec![Claim {
                resource: Resource::File(canonical_root),
                write: true,
            }],
            cancel_rx.clone(),
            tokio::time::Instant::now() + std::time::Duration::from_secs(60),
        )
        .await
        .map_err(|error| match error {
            crate::agent::rig_ext::r#loop::resources::AcquireError::QueueTimeout { .. } => {
                anyhow::anyhow!("{}", error.timeout_message())
            }
            other => anyhow::anyhow!("工作流工作区资源租约获取失败：{other:?}"),
        })?;

    let settings = tokio::task::spawn_blocking({
        let db = services.db.clone();
        move || db.get_settings_v2()
    })
    .await
    .context("读取设置任务失败")??;
    // 工作流定义 v4 起模型目录是静态 ACP 目录（default/sonnet/opus/haiku），
    // 不再依赖应用模型库与 MCP/内置工具发现。
    let catalog = build_harness_catalog();
    // 种子键：plan 当前 state 里的键（普通工作流为空；修复工作流/续跑携带继承或既有键）。
    // 解析失败必须显式报错而非静默退化为空集：空种子键会让 validate 把依赖
    // 继承键的节点误报为 missing-input（错误指向工作流定义而非真正的 state 损坏），
    // 且损坏的 state_json 会在下方被再次解析成空 Map，让续跑丢失既有共享 state。
    let plan_state: Map<String, Value> =
        serde_json::from_str(&plan.state_json).map_err(|error| {
            anyhow::anyhow!(
                "工作流计划共享 state 已损坏（plan_id={}，JSON 解析失败：{error}），无法运行",
                plan.id
            )
        })?;
    let seeded_keys = plan_state.keys().cloned().collect::<HashSet<String>>();
    validate_workflow(&definition, &catalog, &seeded_keys).map_err(anyhow::Error::msg)?;

    // resume 时 DB 已含复制的 cached succeeded 行：只对缺失节点写 pending。
    let existing_runs = store.list_node_runs_async(&run.id).await?;
    let initial_status = existing_runs
        .iter()
        .map(|record| (record.node_id.clone(), record.status.clone()))
        .collect::<HashMap<_, _>>();
    for node in &definition.nodes {
        if !initial_status.contains_key(&node.id) {
            store
                .save_node_run_async(&WorkflowNodeRunRecord::pending(&run.id, &plan_id, node))
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
        harnesses.insert(node.id.clone(), resolve_node_harness(node)?);
    }

    emit_plan_updated(app, &plan_id, &workspace_id);
    emit_run_event(
        app,
        &plan_id,
        &run.id,
        &workspace_id,
        WorkflowRunEvent::RunStarted {
            title: definition.title.clone(),
            attempt_no: run.attempt_no,
            node_count: definition.nodes.len(),
        },
    );

    // v3：需求以提交时快照为准；快照为空时兜底取最新消息（防御旧数据）。
    let user_requirement = crate::agent::rig_ext::agents::plan_access::resolve_user_requirement(
        &services.db,
        &workspace_id,
        &plan.requirement,
    )
    .await;
    // 全局权限审查材料：工作流摘要与审查模型配置每个 run 构建一次（Arc 共享），
    // 节点级快照在派发点按节点生成。
    let review_shared = Arc::new(WorkflowReviewShared {
        config: settings.review.clone(),
        workflow_digest: build_workflow_digest(&definition, &user_requirement),
    });
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
            let node_id = if !checkpoint_passed && settings.workflow.pause_before_write {
                ready
                    .iter()
                    .find(|id| {
                        node_by_id
                            .get(*id)
                            .map(|n| !node_may_write(n))
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
            if !checkpoint_passed && settings.workflow.pause_before_write && node_may_write(&node) {
                emit_run_event(
                    app,
                    &plan_id,
                    &run.id,
                    &workspace_id,
                    WorkflowRunEvent::RunPaused {
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
                    WorkflowRunEvent::RunResumed {},
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
            let review = Arc::new(PermissionReviewMaterials::new(
                Arc::clone(&review_shared),
                &node,
            ));
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
                    acp: settings.workflow.acp.clone(),
                    store: store.clone(),
                    cancel_rx: cancel_rx.clone(),
                    review,
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
                    "[workflow] 节点任务异常（{plan_id}）：{error}；停止派发，等待在途任务结算"
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
                    WorkflowRunEvent::NodeFinished {
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
                    task_error = Some(error.context("持久化工作流共享状态失败"));
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
                        WorkflowRunEvent::NodeFailed {
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
                        WorkflowRunEvent::NodeFailed {
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
    // execute_workflow_run 置失败态、广播事件并兜底中断运行记录）。
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
            WorkflowRunEvent::RunCancelled {},
        );
        return Ok(());
    }

    // 防御：理论上循环退出时不应有未结算节点（失败已传递 skip）；若有则兜底跳过。
    // 触发即状态机 bug，留痕供事后定位（静默落库会掩盖根因）。
    if queue.has_unsettled() {
        for remaining in queue.cancel_remaining() {
            eprintln!(
                "[workflow] run 收尾发现未结算节点（plan={plan_id} run={} node={remaining}），已兜底跳过",
                run.id
            );
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

    // 闭环收尾：验收 → 结果组装 → 回执 → 终态落库。
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
        eprintln!("[workflow] 写入验收结论失败（{plan_id}）：{error:#}");
    }

    let result = resolve_run_result(&definition, &node_runs);
    if let Err(error) = store.update_run_result_async(&run.id, result.clone()).await {
        // 结果落库失败不阻断终态收尾：面板退化为无结果展示，节点输出仍完整。
        eprintln!("[workflow] 写入执行结果失败（{plan_id}）：{error:#}");
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
            result: &result,
        },
    )
    .await;

    emit_run_event(
        app,
        &plan_id,
        &run.id,
        &workspace_id,
        WorkflowRunEvent::RunFinished {
            state: Value::Object(state),
            failed_nodes,
            skipped_nodes,
        },
    );
    Ok(())
}

/// run 收尾组装执行结果（workflow_runs 执行结果列），纯函数便于单测：
/// - 结论节点：工作流定义的汇点（未被任何节点 depends_on 引用者）。唯一汇点即
///   结论节点；多汇点取其中 finished_at 最晚的成功节点（软提示引导编排器以
///   单一汇总节点收口，此规则为未收口工作流兜底；无可选成功汇点时不指定）。
///   结论节点未成功或输出为空 → conclusion_md 为 None（UI 退化为验收 + 文件）。
/// - 结果类型：任一成功节点为 coding 工具组（含 resume 复用 cached 行）→
///   edit（执行写入类）；有成功节点但全为 read_only → review（调研审查类）；
///   无成功节点 → none（执行完成、无结果；unknown 保留给未收尾/历史行，
///   读取层对 unknown 呈现为 result=None）。
/// - 修改文件清单：本次 run 全部节点 affected_files 的并集（排序去重）。
pub(crate) fn resolve_run_result(
    definition: &WorkflowDefinition,
    node_runs: &[WorkflowNodeRunRecord],
) -> WorkflowRunResult {
    let run_by_node: HashMap<&str, &WorkflowNodeRunRecord> = node_runs
        .iter()
        .map(|record| (record.node_id.as_str(), record))
        .collect();

    // 汇点 = 未被任何 depends_on 引用的节点（出度 0）。
    let mut has_downstream: HashSet<&str> = HashSet::new();
    for node in &definition.nodes {
        for dep in &node.depends_on {
            has_downstream.insert(dep.as_str());
        }
    }
    let conclusion_node_id = {
        let sinks: Vec<&WorkflowNode> = definition
            .nodes
            .iter()
            .filter(|node| !has_downstream.contains(node.id.as_str()))
            .collect();
        match sinks.len() {
            0 => None,
            1 => Some(sinks[0].id.clone()),
            _ => sinks
                .iter()
                .filter_map(|node| run_by_node.get(node.id.as_str()).copied())
                .filter(|record| record.status == NODE_SUCCEEDED)
                // finished_at 缺失（异常行）按 0 参与比较，不参与「最晚」竞争。
                .max_by_key(|record| record.finished_at.unwrap_or(0))
                .map(|record| record.node_id.clone()),
        }
    };

    let conclusion_md = conclusion_node_id
        .as_deref()
        .and_then(|node_id| run_by_node.get(node_id).copied())
        .filter(|record| record.status == NODE_SUCCEEDED)
        .map(|record| record.output_text.clone())
        .filter(|output| !output.trim().is_empty());

    let mut has_succeeded = false;
    let mut has_succeeded_coding = false;
    let mut modified_files = std::collections::BTreeSet::new();
    for record in node_runs {
        if record.status == NODE_SUCCEEDED {
            has_succeeded = true;
            if record.base_tool_group == BaseToolGroup::Coding.as_str() {
                has_succeeded_coding = true;
            }
        }
        modified_files.extend(record.affected_files.iter().cloned());
    }
    let result_kind = if has_succeeded_coding {
        RESULT_KIND_EDIT
    } else if has_succeeded {
        RESULT_KIND_REVIEW
    } else {
        RESULT_KIND_NONE
    };

    WorkflowRunResult {
        conclusion_node_id,
        conclusion_md,
        result_kind: result_kind.to_string(),
        modified_files: modified_files.into_iter().collect(),
    }
}

async fn resolve_workspace_root(db: &DispatcherDb, workspace_id: &str) -> Result<PathBuf> {
    let project_id = db
        .get_session_project_id_async(workspace_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("找不到工作流计划所属会话：{workspace_id}"))?;
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
        anyhow::anyhow!("无法定位工作流计划所属项目路径（项目 {project_id} 可能已被删除）")
    })?;
    // 规范化工作区根（解析符号链接与 ../）：后续执行器消息、受影响文件
    // 校验都以此为基准，目录不存在时 fail-closed。
    let root = tokio::task::spawn_blocking(move || path.canonicalize())
        .await
        .context("规范化工作区路径任务失败")?
        .with_context(|| format!("工作区路径不存在或无法访问：{project_id}"))?;
    Ok(root)
}

#[cfg(test)]
mod result_tests {
    use super::*;
    use crate::agent::workflow::types::{NODE_RUNNING, NODE_SKIPPED};

    fn node(id: &str, group: BaseToolGroup, deps: &[&str]) -> WorkflowNode {
        WorkflowNode {
            id: id.into(),
            title: id.into(),
            role: String::new(),
            model_ref: "m1".into(),
            base_tool_group: group,
            task: "task".into(),
            depends_on: deps.iter().map(|dep| dep.to_string()).collect(),
            inject_state_keys: vec![],
            output_key: format!("out_{id}"),
            expected_files: vec![],
            export_policy: Default::default(),
            use_plan_mode: false,
        }
    }

    fn definition(nodes: Vec<WorkflowNode>) -> WorkflowDefinition {
        WorkflowDefinition {
            version: 4,
            title: "测试工作流".into(),
            summary: String::new(),
            state_keys: vec![],
            nodes,
            inherits_from: None,
        }
    }

    fn record(node: &WorkflowNode, status: &str) -> WorkflowNodeRunRecord {
        let mut record = WorkflowNodeRunRecord::pending("run", "plan", node);
        record.status = status.into();
        record
    }

    /// 链式工作流 n1(只读) → n2(coding) → n3(汇总)：唯一汇点 n3 的输出即结论，
    /// coding 节点成功 → edit；文件清单为各节点并集且排序去重。
    #[test]
    fn single_sink_conclusion_and_edit_kind() {
        let def = definition(vec![
            node("n1", BaseToolGroup::ReadOnly, &[]),
            node("n2", BaseToolGroup::Coding, &["n1"]),
            node("n3", BaseToolGroup::ReadOnly, &["n2"]),
        ]);
        let n1 = &def.nodes[0];
        let n2 = &def.nodes[1];
        let n3 = &def.nodes[2];
        let mut r1 = record(n1, NODE_SUCCEEDED);
        r1.affected_files = vec!["docs/report.md".into()];
        let mut r2 = record(n2, NODE_SUCCEEDED);
        r2.affected_files = vec!["src/b.rs".into(), "src/a.rs".into()];
        let mut r3 = record(n3, NODE_SUCCEEDED);
        r3.output_text = "## 审查结论\n- 问题 A".into();
        r3.affected_files = vec!["src/a.rs".into()];

        let result = resolve_run_result(&def, &[r1, r2, r3]);
        assert_eq!(result.conclusion_node_id.as_deref(), Some("n3"));
        assert!(result
            .conclusion_md
            .as_deref()
            .unwrap()
            .contains("审查结论"));
        assert_eq!(result.result_kind, RESULT_KIND_EDIT);
        assert_eq!(
            result.modified_files,
            vec![
                "docs/report.md".to_string(),
                "src/a.rs".to_string(),
                "src/b.rs".to_string()
            ]
        );
    }

    /// 纯 read_only 成功工作流 → review；多汇点时取 finished_at 最晚的成功汇点。
    #[test]
    fn multi_sink_picks_latest_succeeded_and_review_kind() {
        let def = definition(vec![
            node("investigate", BaseToolGroup::ReadOnly, &[]),
            node("audit_a", BaseToolGroup::ReadOnly, &["investigate"]),
            node("audit_b", BaseToolGroup::ReadOnly, &["investigate"]),
        ]);
        let a = &def.nodes[1];
        let b = &def.nodes[2];
        let mut ra = record(a, NODE_SUCCEEDED);
        ra.finished_at = Some(1_000);
        ra.output_text = "早完成的分支结论".into();
        let mut rb = record(b, NODE_SUCCEEDED);
        rb.finished_at = Some(2_000);
        rb.output_text = "## 最终结论".into();

        let result = resolve_run_result(&def, &[ra, rb]);
        assert_eq!(result.conclusion_node_id.as_deref(), Some("audit_b"));
        assert!(result
            .conclusion_md
            .as_deref()
            .unwrap()
            .contains("最终结论"));
        assert_eq!(result.result_kind, RESULT_KIND_REVIEW);
    }

    /// 结论节点失败：conclusion_node_id 仍指向汇点（解释结论缺失原因），
    /// conclusion_md 为 None；成功 coding 节点仍在 → edit。
    #[test]
    fn failed_conclusion_node_yields_no_md() {
        let def = definition(vec![
            node("fix", BaseToolGroup::Coding, &[]),
            node("summary", BaseToolGroup::ReadOnly, &["fix"]),
        ]);
        let mut rf = record(&def.nodes[0], NODE_SUCCEEDED);
        rf.affected_files = vec!["src/a.rs".into()];
        let rs = record(&def.nodes[1], NODE_FAILED);

        let result = resolve_run_result(&def, &[rf, rs]);
        assert_eq!(result.conclusion_node_id.as_deref(), Some("summary"));
        assert!(result.conclusion_md.is_none());
        assert_eq!(result.result_kind, RESULT_KIND_EDIT);
        assert_eq!(result.modified_files, vec!["src/a.rs".to_string()]);
    }

    /// 无成功节点（全失败/跳过/未运行）→ none（已组装但无结果），多汇点无可选
    /// 成功节点时不指定结论节点。
    #[test]
    fn no_succeeded_nodes_yields_none_kind() {
        let def = definition(vec![
            node("a", BaseToolGroup::Coding, &[]),
            node("b", BaseToolGroup::Coding, &[]),
        ]);
        let ra = record(&def.nodes[0], NODE_FAILED);
        let rb = record(&def.nodes[1], NODE_SKIPPED);
        let result = resolve_run_result(&def, &[ra, rb]);
        assert_eq!(result.result_kind, RESULT_KIND_NONE);
        assert!(result.conclusion_node_id.is_none());
        assert!(result.conclusion_md.is_none());

        // 跳过（未运行）不算成功，即使汇总节点行存在也不能产出结论。
        let def2 = definition(vec![
            node("work", BaseToolGroup::Coding, &[]),
            node("summary", BaseToolGroup::ReadOnly, &["work"]),
        ]);
        let rw = record(&def2.nodes[0], NODE_RUNNING);
        let rs = record(&def2.nodes[1], NODE_SKIPPED);
        let result2 = resolve_run_result(&def2, &[rw, rs]);
        assert!(result2.conclusion_md.is_none());
        assert_eq!(result2.result_kind, RESULT_KIND_NONE);
    }
}

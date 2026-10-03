//! 图编排 Tauri 命令：计划查询 / 编辑 / 启动 / 取消。

use futures::FutureExt;
use tauri::{AppHandle, Manager, State};

use super::harness::build_harness_catalog;
use super::runner::{emit_plan_updated, execute_graph_run, GraphRunServices};
use super::store::GraphStore;
use super::types::{
    GraphDefinition, GraphHarnessCatalog, GraphPlanRecord, GraphPlanSummaryItem, GraphRunDetail,
    GraphRunSummary, PLAN_CANCELLED, PLAN_COMPLETED, PLAN_DRAFT, PLAN_FAILED, PLAN_RUNNING,
    RUN_MODE_FULL, RUN_MODE_RESUME,
};
use super::validate::validate_graph;
use crate::agent::state::DispatcherState;

/// 读取图计划（含 node_runs + state，用于面板回放）。
#[tauri::command]
pub async fn graph_plan_get(
    state: State<'_, DispatcherState>,
    plan_id: String,
) -> Result<GraphPlanRecord, String> {
    let store = GraphStore::new(state.db());
    store
        .get_plan_async(&plan_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("图计划不存在：{plan_id}"))
}

/// 会话最近一次更新的图计划（会话头部入口）。
#[tauri::command]
pub async fn graph_plan_latest_for_session(
    state: State<'_, DispatcherState>,
    workspace_id: String,
) -> Result<Option<GraphPlanRecord>, String> {
    let store = GraphStore::new(state.db());
    store
        .latest_plan_for_workspace_async(&workspace_id)
        .await
        .map_err(|error| error.to_string())
}

/// 会话全部图计划的轻量列表（图列表页数据源，不含定义/state 大字段）。
#[tauri::command]
pub async fn graph_plan_list_for_session(
    state: State<'_, DispatcherState>,
    workspace_id: String,
) -> Result<Vec<GraphPlanSummaryItem>, String> {
    let store = GraphStore::new(state.db());
    store
        .list_plan_summaries_for_workspace_async(&workspace_id)
        .await
        .map_err(|error| error.to_string())
}

/// draft 图定义更新的共享编排（前端 `graph_plan_update` 命令与编排器
/// `graph_node_update` 协议拦截共用）：draft 双检 + 整图校验 + store 条件
/// 更新 + 广播。调用方负责先解析/构造 `GraphDefinition`（含 normalize_ids）。
/// 错误文本面向调用方可读（校验问题/状态门禁），内部故障也一并字符串化，
/// 与命令层既有口径一致。
pub(crate) async fn apply_draft_definition_update(
    store: &GraphStore,
    app: Option<&AppHandle>,
    plan: &super::types::GraphPlanRecord,
    definition: &GraphDefinition,
) -> Result<(), String> {
    let plan_id = plan.id.clone();
    if plan.status != PLAN_DRAFT {
        return Err(format!(
            "当前状态（{}）不允许编辑图定义；仅 draft 态可编辑",
            plan.status
        ));
    }

    let catalog = build_harness_catalog();
    // 种子键沿用 plan 当前 state（draft 态普通图为空，修复图为继承 state）。
    // 解析失败必须显式报错：在空种子键前提下校验会把本应合法的修复图
    // injectStateKeys 误报为「不在继承的共享 state 中」，且掩盖真实根因。
    let seeded_keys =
        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&plan.state_json)
            .map_err(|error| {
                format!("图计划共享 state 已损坏（JSON 解析失败：{error}），无法校验图定义")
            })?
            .keys()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
    validate_graph(definition, &catalog, &seeded_keys)?;

    // 写入前重读状态：前置检查与这里之间隔着目录刷新/校验等多个 await，
    // 若 graph_run_start 并发把计划置为 running，必须放弃写入。store 层的
    // 条件更新（AND status='draft' + 影响行数检查）是最终门禁。
    let plan_now = store
        .get_plan_async(&plan_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("图计划不存在：{plan_id}"))?;
    if plan_now.status != PLAN_DRAFT {
        return Err(format!(
            "当前状态（{}）不允许编辑图定义；仅 draft 态可编辑",
            plan_now.status
        ));
    }
    store
        .update_plan_definition_async(&plan_id, plan_now.updated_at, definition)
        .await
        .map_err(|error| error.to_string())?;
    if let Some(app) = app {
        emit_plan_updated(app, &plan_id, &plan.workspace_id);
    }
    Ok(())
}

/// 用户确认前编辑图定义（仅 draft 态允许；更新时重新校验）。
#[tauri::command]
pub async fn graph_plan_update(
    app: AppHandle,
    state: State<'_, DispatcherState>,
    plan_id: String,
    definition_json: String,
) -> Result<(), String> {
    let store = GraphStore::new(state.db());
    let plan = store
        .get_plan_async(&plan_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("图计划不存在：{plan_id}"))?;

    let mut definition: GraphDefinition = serde_json::from_str(&definition_json)
        .map_err(|error| format!("错误：definition_json 不是合法的图定义：{error}"))?;
    definition.normalize_ids();

    apply_draft_definition_update(&store, Some(&app), &plan, &definition).await
}

/// 确认执行 / 重新执行：draft/failed/cancelled/completed 态允许；置 running 后
/// 异步执行图运行器。`mode`：full（默认，完整执行）/ resume（断点续跑，仅
/// failed/cancelled 态可，用最近一次运行的成功节点与 state 起步）。
#[tauri::command]
pub async fn graph_run_start(
    app: AppHandle,
    state: State<'_, DispatcherState>,
    plan_id: String,
    mode: Option<String>,
) -> Result<(), String> {
    let store = GraphStore::new(state.db());
    let plan = store
        .get_plan_async(&plan_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("图计划不存在：{plan_id}"))?;
    if !matches!(
        plan.status.as_str(),
        PLAN_DRAFT | PLAN_FAILED | PLAN_CANCELLED | PLAN_COMPLETED
    ) {
        return Err(format!(
            "当前状态（{}）不允许启动执行；仅 draft/failed/cancelled/completed 可启动",
            plan.status
        ));
    }

    let mode = match mode.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        None | Some(RUN_MODE_FULL) => RUN_MODE_FULL.to_string(),
        Some(RUN_MODE_RESUME) => {
            // 断点续跑仅对 failed/cancelled 有意义，且需存在已终态的历史运行。
            if !matches!(plan.status.as_str(), PLAN_FAILED | PLAN_CANCELLED) {
                return Err("仅 failed/cancelled 态的图支持断点续跑".to_string());
            }
            let latest = store
                .get_latest_run_async(&plan_id)
                .await
                .map_err(|error| error.to_string())?;
            let Some(latest) = latest else {
                return Err("没有可续跑的历史运行".to_string());
            };
            if !matches!(latest.status.as_str(), PLAN_FAILED | PLAN_CANCELLED) {
                return Err(format!("最近一次运行状态为 {}，无法续跑", latest.status));
            }
            RUN_MODE_RESUME.to_string()
        }
        // 未知取值（拼写错误等）显式报错：若静默回退为 full，本意断点续跑的
        // 调用会触发完整重跑——共享 state 被重置、写节点全部重新执行，且无提示。
        Some(unknown) => {
            return Err(format!(
                "未知的执行模式：{unknown}；仅支持 {RUN_MODE_FULL} / {RUN_MODE_RESUME}"
            ))
        }
    };

    let handle = state.begin_graph_run(&plan_id)?;
    // 先置 running 再 spawn（命令返回后前端立即查询也能看到正确状态）；
    // 运行器内部会再次写入并广播 graph-plan-updated。
    if let Err(error) = store.update_plan_status_async(&plan_id, PLAN_RUNNING).await {
        state.finish_graph_run(&plan_id);
        return Err(error.to_string());
    }
    let services = GraphRunServices {
        db: state.db().clone(),
        agent_config: state.agent_config(),
    };
    let run_plan_id = plan_id.clone();
    let run_app = app.clone();
    tokio::spawn(async move {
        // catch_unwind 兜底：任何情况下都释放运行槽位。
        let result = std::panic::AssertUnwindSafe(execute_graph_run(
            run_app.clone(),
            services,
            run_plan_id.clone(),
            mode,
            handle,
        ))
        .catch_unwind()
        .await;
        if let Err(error) = result {
            eprintln!("[graph] 图运行器 panic（{run_plan_id}）：{error:?}");
            let store = GraphStore::new(run_app.state::<DispatcherState>().db());
            if let Err(store_error) = store.fail_interrupted_runs_async(Some(&run_plan_id)).await {
                eprintln!("[graph] panic 后恢复中断运行状态失败（{run_plan_id}）：{store_error:#}");
            }
            // 无论 fail_interrupted_runs 是否成功/生效（panic 早于 create_run 时
            // 没有 running 的运行行可更新，计划仍停留在 running），都显式把仍处
            // running 的计划复位为 failed 并广播 graph-plan-updated——否则计划
            // 长期卡在 running 且前端无任何事件可感知异常、无法驱动重试。
            match store.get_plan_async(&run_plan_id).await {
                Ok(Some(plan)) => {
                    if plan.status == PLAN_RUNNING {
                        if let Err(store_error) = store
                            .update_plan_status_async(&run_plan_id, PLAN_FAILED)
                            .await
                        {
                            eprintln!(
                                "[graph] panic 后复位计划状态失败（{run_plan_id}）：{store_error:#}"
                            );
                        }
                    }
                    emit_plan_updated(&run_app, &run_plan_id, &plan.workspace_id);
                }
                Ok(None) => {
                    eprintln!("[graph] panic 后计划不存在（{run_plan_id}），跳过事件广播")
                }
                Err(error) => {
                    eprintln!("[graph] panic 后读取计划失败（{run_plan_id}）：{error:#}")
                }
            }
            run_app
                .state::<DispatcherState>()
                .finish_graph_run(&run_plan_id);
            return;
        }
        run_app
            .state::<DispatcherState>()
            .finish_graph_run(&run_plan_id);
    });
    Ok(())
}

/// 请求取消运行中的图：通知节点执行器 abort，由节点任务自行结算为 cancelled。
#[tauri::command]
pub async fn graph_run_cancel(
    app: AppHandle,
    state: State<'_, DispatcherState>,
    plan_id: String,
) -> Result<bool, String> {
    let cancelled = state.cancel_graph_run(&plan_id);
    if !cancelled {
        // 运行槽位不存在但状态卡在 running（如应用重启后的残留）：直接复位为 cancelled。
        // 复位是自愈兜底而非取消主路径：失败不再静默吞掉（留痕可观测），
        // 但也不把整条取消命令报错——取消本身对活动 run 已无更多可做。
        let store = GraphStore::new(state.db());
        if let Ok(Some(plan)) = store.get_plan_async(&plan_id).await {
            if plan.status == PLAN_RUNNING {
                if let Err(error) = store
                    .update_plan_status_async(&plan_id, super::types::PLAN_CANCELLED)
                    .await
                {
                    eprintln!("[graph] 残留 running 计划复位失败（{plan_id}）：{error:#}");
                } else {
                    emit_plan_updated(&app, &plan_id, &plan.workspace_id);
                }
            }
        }
    }
    Ok(cancelled)
}

/// 恢复暂停中（高危写检查点）的图运行。
#[tauri::command]
pub async fn graph_run_resume(
    app: AppHandle,
    state: State<'_, DispatcherState>,
    plan_id: String,
) -> Result<bool, String> {
    let resumed = state.resume_graph_run(&plan_id);
    if resumed {
        // resumed 已为 true：查询失败不改变返回值，但需可观测——
        // 否则前端收不到 graph-plan-updated，UI 状态不刷新且无线索。
        match GraphStore::new(state.db()).get_plan_async(&plan_id).await {
            Ok(Some(plan)) => emit_plan_updated(&app, &plan_id, &plan.workspace_id),
            Ok(None) => eprintln!("[graph] resume 后计划不存在（{plan_id}），跳过事件广播"),
            Err(error) => eprintln!("[graph] resume 后读取计划失败（{plan_id}）：{error:#}"),
        }
    }
    Ok(resumed)
}

#[tauri::command]
pub async fn graph_harness_catalog_get(
    state: State<'_, DispatcherState>,
    _workspace_id: String,
) -> Result<GraphHarnessCatalog, String> {
    // 图定义 v4 起目录为静态 ACP 模型表；workspace_id 保留为命令契约。
    let mut catalog = build_harness_catalog();
    // 诊断：凭据缺失时提示节点执行将依赖本机登录态（与 acp_exec::build_launch 同口径）。
    // 设置读取失败不阻断目录返回（编辑草稿不该被设置库故障卡死）。
    let db = state.db().clone();
    let settings = tokio::task::spawn_blocking(move || db.get_settings_v2())
        .await
        .ok()
        .and_then(Result::ok);
    // 仅在设置成功读取且确无 key 时提示；读取失败不做凭据断言。
    if let Some(settings) = settings {
        let has_key = settings
            .graph
            .acp
            .api_key
            .as_deref()
            .map(str::trim)
            .is_some_and(|key| !key.is_empty());
        if !has_key {
            catalog.diagnostics.push(
                "未配置 ACP API Key：节点执行将使用本机 ~/.claude 登录态（可在 设置 → 执行图 中配置）".to_string(),
            );
        }
    }
    Ok(catalog)
}

#[tauri::command]
pub async fn graph_run_get(
    state: State<'_, DispatcherState>,
    run_id: String,
) -> Result<GraphRunDetail, String> {
    GraphStore::new(state.db())
        .get_run_detail_async(&run_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("图运行不存在：{run_id}"))
}

/// 对已收尾的 run 重新执行验收：验收模型配置修复后补救 unknown 结论
/// （如「验收模型调用失败」），或对既有结论复检。重跑 fail-safe 的
/// verify_run → 更新 run 验收结论 → 投递简短回执 → 广播计划更新。
/// 仅允许对终态 run 重验收；运行中的 run 由收尾路径自然验收。
#[tauri::command]
pub async fn graph_run_reverify(
    app: AppHandle,
    state: State<'_, DispatcherState>,
    run_id: String,
) -> Result<GraphRunSummary, String> {
    let db = state.db().clone();
    let store = GraphStore::new(&db);
    let detail = store
        .get_run_detail_async(&run_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("图运行不存在：{run_id}"))?;
    let run = &detail.run;
    if !matches!(
        run.status.as_str(),
        PLAN_COMPLETED | PLAN_FAILED | PLAN_CANCELLED
    ) {
        return Err(format!(
            "运行尚未结束（当前状态 {}），仅已收尾的运行支持重新验收",
            run.status
        ));
    }
    let plan = store
        .get_plan_async(&run.plan_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("图计划不存在：{}", run.plan_id))?;
    let mut definition: GraphDefinition = serde_json::from_str(&plan.definition_json)
        .map_err(|error| format!("错误：图定义已损坏（JSON 解析失败：{error}），无法重新验收"))?;
    definition.normalize_ids();
    let state_map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&plan.state_json).map_err(|error| {
            format!("错误：图计划共享 state 已损坏（JSON 解析失败：{error}），无法重新验收")
        })?;

    let settings = tokio::task::spawn_blocking({
        let db = db.clone();
        move || db.get_settings_v2()
    })
    .await
    .map_err(|error| format!("读取设置任务失败：{error}"))?
    .map_err(|error| format!("读取设置失败：{error}"))?;
    let agent_config = state.agent_config();

    // 需求与运行收尾同源：以提交时快照为准，快照为空的旧数据兜底取最新消息。
    let mut requirement = plan.requirement.trim().to_string();
    if requirement.is_empty() {
        requirement = db
            .get_latest_user_message_content_async(&plan.workspace_id)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
    }

    let started = std::time::Instant::now();
    let verdict = super::verifier::verify_run(
        &agent_config,
        &settings,
        &requirement,
        &definition,
        &state_map,
        &detail.node_runs,
    )
    .await;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    store
        .update_run_verdict_async(&run_id, &verdict.status, &verdict.reason)
        .await
        .map_err(|error| format!("写入验收结论失败：{error}"))?;
    super::receipt::deliver_reverify_note(
        &app,
        &db,
        &plan.workspace_id,
        &plan,
        run,
        &verdict,
        elapsed_ms,
    )
    .await;
    emit_plan_updated(&app, &plan.id, &plan.workspace_id);
    Ok(GraphRunSummary {
        verdict_status: verdict.status.clone(),
        verdict_reason: verdict.reason.clone(),
        ..run.clone()
    })
}

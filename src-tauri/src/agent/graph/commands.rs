//! 图编排 Tauri 命令：计划查询 / 编辑 / 启动 / 取消。

use futures::FutureExt;
use tauri::{AppHandle, Manager, State};

use super::harness::build_harness_catalog;
use super::runner::{emit_plan_updated, execute_graph_run, GraphRunServices};
use super::store::GraphStore;
use super::types::{
    GraphDefinition, GraphHarnessCatalog, GraphPlanRecord, GraphRunDetail, PLAN_CANCELLED,
    PLAN_COMPLETED, PLAN_DRAFT, PLAN_FAILED, PLAN_RUNNING, RUN_MODE_FULL, RUN_MODE_RESUME,
};
use super::validate::validate_graph;
use crate::agent::state::DispatcherState;
use crate::agent::tools::ToolRegistry;

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
    if plan.status != PLAN_DRAFT {
        return Err(format!(
            "当前状态（{}）不允许编辑图定义；仅 draft 态可编辑",
            plan.status
        ));
    }

    let mut definition: GraphDefinition = serde_json::from_str(&definition_json)
        .map_err(|error| format!("错误：definition_json 不是合法的图定义：{error}"))?;
    definition.normalize_ids();

    let catalog = catalog_for_workspace(&state, &plan.workspace_id).await?;
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
    validate_graph(&definition, &catalog, &seeded_keys)?;

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
        .update_plan_definition_async(&plan_id, &definition)
        .await
        .map_err(|error| error.to_string())?;
    emit_plan_updated(&app, &plan_id, &plan.workspace_id);
    Ok(())
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
        mcp_registry: state.mcp_registry(),
        ssh_manager: state.ssh_manager(),
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

/// 请求取消运行中的图：PI sidecar 先 abort，超时后终止进程组。
#[tauri::command]
pub async fn graph_run_cancel(
    app: AppHandle,
    state: State<'_, DispatcherState>,
    plan_id: String,
) -> Result<bool, String> {
    let cancelled = state.cancel_graph_run(&plan_id);
    if !cancelled {
        // 运行槽位不存在但状态卡在 running（如应用重启后的残留）：直接复位为 cancelled。
        let store = GraphStore::new(state.db());
        if let Ok(Some(plan)) = store.get_plan_async(&plan_id).await {
            if plan.status == PLAN_RUNNING {
                let _ = store
                    .update_plan_status_async(&plan_id, super::types::PLAN_CANCELLED)
                    .await;
                emit_plan_updated(&app, &plan_id, &plan.workspace_id);
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
    workspace_id: String,
) -> Result<GraphHarnessCatalog, String> {
    catalog_for_workspace(&state, &workspace_id).await
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

pub(crate) async fn catalog_for_workspace(
    state: &DispatcherState,
    workspace_id: &str,
) -> Result<GraphHarnessCatalog, String> {
    let project_id = state
        .db()
        .get_session_project_id_async(workspace_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("找不到图计划所属会话：{workspace_id}"))?;
    let workspace = {
        let db = state.db().clone();
        let lookup = project_id.clone();
        tokio::task::spawn_blocking(move || db.find_project(&lookup).ok().flatten())
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "无法定位图计划所属项目".to_string())?
            .path
    };
    // projects 表中的路径可能带符号链接/相对成分：统一经 McpScope::project
    // canonicalize，保证与图执行共用同一缓存键（修复旧实现键漂移）。
    let mcp_scope = crate::mcp::McpScope::project(std::path::Path::new(&workspace))?;
    state.mcp_registry().ensure_recent(&mcp_scope).await?;
    let settings = tokio::task::spawn_blocking({
        let db = state.db().clone();
        move || db.get_settings_v2()
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
    let registry = ToolRegistry::default_tools(state.mcp_registry(), state.ssh_manager());
    Ok(build_harness_catalog(
        std::path::Path::new(&workspace),
        &mcp_scope,
        &settings,
        &registry,
    )
    .await)
}

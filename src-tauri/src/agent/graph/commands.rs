//! 图编排 Tauri 命令：计划查询 / 编辑 / 启动 / 取消。

use futures::FutureExt;
use tauri::{AppHandle, Manager, State};

use super::harness::build_harness_catalog;
use super::runner::{emit_plan_updated, execute_graph_run, GraphRunServices};
use super::store::GraphStore;
use super::types::{
    GraphDefinition, GraphHarnessCatalog, GraphPlanRecord, GraphRunDetail, PLAN_CANCELLED,
    PLAN_COMPLETED, PLAN_CONFIRMED, PLAN_DRAFT, PLAN_FAILED, PLAN_RUNNING,
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

    let definition: GraphDefinition = serde_json::from_str(&definition_json)
        .map_err(|error| format!("错误：definition_json 不是合法的图定义：{error}"))?;

    let catalog = catalog_for_workspace(&state, &plan.workspace_id).await?;
    validate_graph(&definition, &catalog)?;

    store
        .update_plan_definition_async(&plan_id, &definition)
        .await
        .map_err(|error| error.to_string())?;
    emit_plan_updated(&app, &plan_id, &plan.workspace_id);
    Ok(())
}

/// 确认执行 / 重新执行：draft/confirmed/failed/cancelled/completed 态允许；
/// 置 running 后异步执行图运行器；每次重跑创建独立 run 快照。
#[tauri::command]
pub async fn graph_run_start(
    app: AppHandle,
    state: State<'_, DispatcherState>,
    plan_id: String,
) -> Result<(), String> {
    let store = GraphStore::new(state.db());
    let plan = store
        .get_plan_async(&plan_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("图计划不存在：{plan_id}"))?;
    if !matches!(
        plan.status.as_str(),
        PLAN_DRAFT | PLAN_CONFIRMED | PLAN_FAILED | PLAN_CANCELLED | PLAN_COMPLETED
    ) {
        return Err(format!(
            "当前状态（{}）不允许启动执行；仅 draft/confirmed/failed/cancelled/completed 可启动",
            plan.status
        ));
    }

    let cancel_rx = state.begin_graph_run(&plan_id)?;
    // 先置 running 再 spawn（命令返回后前端立即查询也能看到正确状态）；
    // 运行器内部会再次写入并广播 graph-plan-updated。
    if let Err(error) = store.update_plan_status_async(&plan_id, PLAN_RUNNING).await {
        state.finish_graph_run(&plan_id);
        return Err(error.to_string());
    }
    let services = GraphRunServices {
        db: state.db().clone(),
        agent_config: state.agent_config(),
        project_mcp_registry: state.project_mcp_registry(),
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
            cancel_rx,
        ))
        .catch_unwind()
        .await;
        if let Err(error) = result {
            eprintln!("[graph] 图运行器 panic（{run_plan_id}）：{error:?}");
            let store = GraphStore::new(run_app.state::<DispatcherState>().db());
            if let Err(store_error) = store.fail_interrupted_runs_async(Some(&run_plan_id)).await {
                eprintln!("[graph] panic 后恢复中断运行状态失败（{run_plan_id}）：{store_error:#}");
            }
            if let Ok(Some(plan)) = store.get_plan_async(&run_plan_id).await {
                emit_plan_updated(&run_app, &run_plan_id, &plan.workspace_id);
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
    let workspace = tokio::task::spawn_blocking(move || {
        crate::project::storage::load_projects()
            .ok()
            .and_then(|projects| {
                projects
                    .into_iter()
                    .find(|project| project.id == project_id)
                    .map(|project| project.path)
            })
    })
    .await
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "无法定位图计划所属项目".to_string())?;
    state
        .project_mcp_registry()
        .ensure_recent(std::path::Path::new(&workspace))
        .await?;
    let settings = tokio::task::spawn_blocking({
        let db = state.db().clone();
        move || db.get_settings_v2()
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
    let registry = ToolRegistry::default_tools(state.project_mcp_registry(), state.ssh_manager());
    Ok(build_harness_catalog(std::path::Path::new(&workspace), &settings, &registry).await)
}

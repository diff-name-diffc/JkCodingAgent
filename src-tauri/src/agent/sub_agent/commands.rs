use anyhow::{anyhow, Context};
use tauri::State;

use super::config::SubAgentConfig;
use super::db::{SubAgentRecord, SubAgentRunTraceRecord, ToolInfo};
use crate::agent::DispatcherState;
use crate::shared::error::{CommandResult, IntoCommandResult};

#[tauri::command]
pub async fn sub_agent_list(
    state: State<'_, DispatcherState>,
) -> CommandResult<Vec<SubAgentRecord>> {
    let result = (|| {
        let manager = state
            .sub_agent_manager()
            .ok_or_else(|| anyhow!("子智能体管理器未初始化"))?;
        manager.list_all().context("查询子智能体列表失败")
    })();
    result.into_command_result()
}

#[tauri::command]
pub async fn sub_agent_create(
    state: State<'_, DispatcherState>,
    config_json: String,
) -> CommandResult<SubAgentRecord> {
    let result = (|| {
        let manager = state
            .sub_agent_manager()
            .ok_or_else(|| anyhow!("子智能体管理器未初始化"))?;
        let config: SubAgentConfig =
            serde_json::from_str(&config_json).context("解析子智能体配置失败")?;
        manager.create(config).context("创建子智能体失败")
    })();
    result.into_command_result()
}

#[tauri::command]
pub async fn sub_agent_update(
    state: State<'_, DispatcherState>,
    id: String,
    config_json: String,
) -> CommandResult<SubAgentRecord> {
    let result = (|| {
        let manager = state
            .sub_agent_manager()
            .ok_or_else(|| anyhow!("子智能体管理器未初始化"))?;
        let mut config: SubAgentConfig =
            serde_json::from_str(&config_json).context("解析子智能体配置失败")?;
        config.agent_id = id.clone();
        manager
            .update(&id, config)
            .with_context(|| format!("更新子智能体失败（id={id}）"))
    })();
    result.into_command_result()
}

#[tauri::command]
pub async fn sub_agent_delete(state: State<'_, DispatcherState>, id: String) -> CommandResult<()> {
    let result = (|| {
        let manager = state
            .sub_agent_manager()
            .ok_or_else(|| anyhow!("子智能体管理器未初始化"))?;
        manager
            .delete(&id)
            .with_context(|| format!("删除子智能体失败（id={id}）"))
    })();
    result.into_command_result()
}

#[tauri::command]
pub async fn sub_agent_seed_browser(state: State<'_, DispatcherState>) -> CommandResult<()> {
    let result = (|| {
        let manager = state
            .sub_agent_manager()
            .ok_or_else(|| anyhow!("子智能体管理器未初始化"))?;
        manager
            .seed_browser_force()
            .context("重建内置浏览器子智能体失败")
    })();
    result.into_command_result()
}

#[tauri::command]
pub async fn sub_agent_list_tools(
    state: State<'_, DispatcherState>,
) -> CommandResult<Vec<ToolInfo>> {
    let result = state
        .registered_tool_names()
        .map(|names| {
            names
                .into_iter()
                .map(|(name, description)| ToolInfo { name, description })
                .collect()
        })
        .ok_or_else(|| anyhow!("工具信息未初始化"));
    result.into_command_result()
}

#[tauri::command]
pub async fn sub_agent_set_global_enabled(
    state: State<'_, DispatcherState>,
    sub_agent_ids: Vec<String>,
) -> CommandResult<()> {
    let result = (|| {
        let manager = state
            .sub_agent_manager()
            .ok_or_else(|| anyhow!("子智能体管理器未初始化"))?;
        manager
            .set_global_enabled(&sub_agent_ids)
            .context("保存全局启用子智能体失败")
    })();
    result.into_command_result()
}

#[tauri::command]
pub async fn sub_agent_get_global_enabled(
    state: State<'_, DispatcherState>,
) -> CommandResult<Vec<SubAgentRecord>> {
    let result = (|| {
        let manager = state
            .sub_agent_manager()
            .ok_or_else(|| anyhow!("子智能体管理器未初始化"))?;
        manager
            .get_global_enabled()
            .context("查询全局启用子智能体失败")
    })();
    result.into_command_result()
}

#[tauri::command]
pub async fn sub_agent_get_run_trace(
    state: State<'_, DispatcherState>,
    workspace_id: String,
    tool_call_id: String,
) -> CommandResult<Option<SubAgentRunTraceRecord>> {
    let manager = match state.sub_agent_manager() {
        Some(manager) => manager,
        None => return Err(anyhow!("子智能体管理器未初始化")).into_command_result(),
    };
    let result = tokio::task::spawn_blocking(move || {
        manager
            .get_run_trace(&workspace_id, &tool_call_id)
            .with_context(|| {
                format!(
                    "查询子智能体执行轨迹失败（workspace_id={workspace_id}, tool_call_id={tool_call_id}）"
                )
            })
    })
    .await
    .context("等待子智能体轨迹查询任务失败")
    .and_then(|result| result);
    result.into_command_result()
}

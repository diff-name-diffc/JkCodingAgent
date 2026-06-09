use tauri::State;

use super::config::SubAgentConfig;
use super::db::{SubAgentRecord, ToolInfo};
use crate::agent::DispatcherState;

#[tauri::command]
pub async fn sub_agent_list(
    state: State<'_, DispatcherState>,
) -> Result<Vec<SubAgentRecord>, String> {
    let manager = state.sub_agent_manager().ok_or("子智能体管理器未初始化")?;
    manager.list_all().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn sub_agent_get(
    state: State<'_, DispatcherState>,
    id: String,
) -> Result<SubAgentRecord, String> {
    let manager = state.sub_agent_manager().ok_or("子智能体管理器未初始化")?;
    manager
        .get_record(&id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("子智能体 '{}' 不存在", id))
}

#[tauri::command]
pub async fn sub_agent_create(
    state: State<'_, DispatcherState>,
    config_json: String,
) -> Result<SubAgentRecord, String> {
    let manager = state.sub_agent_manager().ok_or("子智能体管理器未初始化")?;
    let config: SubAgentConfig =
        serde_json::from_str(&config_json).map_err(|e| format!("配置解析失败：{}", e))?;
    manager.create(config).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn sub_agent_update(
    state: State<'_, DispatcherState>,
    id: String,
    config_json: String,
) -> Result<SubAgentRecord, String> {
    let manager = state.sub_agent_manager().ok_or("子智能体管理器未初始化")?;
    let mut config: SubAgentConfig =
        serde_json::from_str(&config_json).map_err(|e| format!("配置解析失败：{}", e))?;
    config.agent_id = id.clone();
    manager.update(&id, config).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn sub_agent_delete(state: State<'_, DispatcherState>, id: String) -> Result<(), String> {
    let manager = state.sub_agent_manager().ok_or("子智能体管理器未初始化")?;
    manager.delete(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn sub_agent_seed_browser(state: State<'_, DispatcherState>) -> Result<(), String> {
    let manager = state.sub_agent_manager().ok_or("子智能体管理器未初始化")?;
    manager.seed_browser_force().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn sub_agent_list_tools(
    state: State<'_, DispatcherState>,
) -> Result<Vec<ToolInfo>, String> {
    state
        .registered_tool_names()
        .map(|names| {
            names
                .into_iter()
                .map(|(name, description)| ToolInfo { name, description })
                .collect()
        })
        .ok_or_else(|| "工具信息未初始化".to_string())
}

#[tauri::command]
pub async fn sub_agent_set_context_enabled(
    state: State<'_, DispatcherState>,
    context: String,
    sub_agent_ids: Vec<String>,
) -> Result<(), String> {
    let manager = state.sub_agent_manager().ok_or("子智能体管理器未初始化")?;
    manager
        .set_context_enabled(&context, &sub_agent_ids)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn sub_agent_get_context_enabled(
    state: State<'_, DispatcherState>,
    context: String,
) -> Result<Vec<SubAgentRecord>, String> {
    let manager = state.sub_agent_manager().ok_or("子智能体管理器未初始化")?;
    manager
        .get_context_enabled(&context)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn sub_agent_set_session_enabled(
    state: State<'_, DispatcherState>,
    session_id: String,
    sub_agent_ids: Vec<String>,
) -> Result<(), String> {
    let manager = state.sub_agent_manager().ok_or("子智能体管理器未初始化")?;
    manager
        .set_session_enabled(&session_id, &sub_agent_ids)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn sub_agent_get_session_enabled(
    state: State<'_, DispatcherState>,
    session_id: String,
) -> Result<Vec<SubAgentRecord>, String> {
    let manager = state.sub_agent_manager().ok_or("子智能体管理器未初始化")?;
    manager
        .get_session_enabled(&session_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn sub_agent_set_global_enabled(
    state: State<'_, DispatcherState>,
    sub_agent_ids: Vec<String>,
) -> Result<(), String> {
    let manager = state.sub_agent_manager().ok_or("子智能体管理器未初始化")?;
    manager
        .set_global_enabled(&sub_agent_ids)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn sub_agent_get_global_enabled(
    state: State<'_, DispatcherState>,
) -> Result<Vec<SubAgentRecord>, String> {
    let manager = state.sub_agent_manager().ok_or("子智能体管理器未初始化")?;
    manager.get_global_enabled().map_err(|e| e.to_string())
}

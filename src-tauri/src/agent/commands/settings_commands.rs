use super::*;

#[tauri::command]
pub async fn aha_get_settings_v2(
    state: tauri::State<'_, DispatcherState>,
) -> Result<AhaSettingsV2, String> {
    state
        .db()
        .get_settings_v2()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn aha_save_settings_v2(
    state: tauri::State<'_, DispatcherState>,
    settings: AhaSettingsV2,
) -> Result<AhaSettingsV2, String> {
    state
        .db()
        .save_settings_v2(&settings)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn aha_list_agent_tools(
    state: tauri::State<'_, DispatcherState>,
    context: String,
) -> Result<Vec<ToolInfo>, String> {
    // 内置工具枚举与工作区无关；MCP 动态工具不进工具清单（启停治理在 MCP 注册表层）。
    let ctx = AgentContext::from_wire(&context).map_err(|e| e.to_string())?;
    state.list_agent_tools(ctx).await
}

#[tauri::command]
pub async fn dispatcher_stop_run(
    state: tauri::State<'_, DispatcherState>,
    browser_manager: tauri::State<'_, BrowserManager>,
    workspace_id: String,
) -> Result<bool, String> {
    let stopped = state.stop_run(&workspace_id);
    let _ = browser_manager.stop(&workspace_id).await;
    Ok(stopped)
}

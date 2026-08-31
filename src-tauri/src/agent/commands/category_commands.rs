use super::*;

#[tauri::command]
pub async fn chat_list_categories(
    state: tauri::State<'_, DispatcherState>,
) -> Result<Vec<ChatCategory>, String> {
    let db = state.db().clone();
    run_dispatcher_db("chat_list_categories", move || db.list_chat_categories()).await
}

#[tauri::command]
pub async fn chat_create_category(
    state: tauri::State<'_, DispatcherState>,
    name: String,
    icon: Option<String>,
    color: Option<String>,
    allowed_tools: Option<Vec<String>>,
    system_prompt: Option<String>,
) -> Result<ChatCategory, String> {
    let db = state.db().clone();
    run_dispatcher_db("chat_create_category", move || {
        db.create_chat_category(
            &name,
            icon.as_deref().unwrap_or(""),
            color.as_deref().unwrap_or(""),
            allowed_tools,
            system_prompt.as_deref(),
        )
    })
    .await
}

#[tauri::command]
pub async fn chat_update_category(
    state: tauri::State<'_, DispatcherState>,
    category_id: String,
    name: Option<String>,
    icon: Option<String>,
    color: Option<String>,
) -> Result<Option<ChatCategory>, String> {
    let db = state.db().clone();
    run_dispatcher_db("chat_update_category", move || {
        db.update_chat_category(
            &category_id,
            name.as_deref(),
            icon.as_deref(),
            color.as_deref(),
        )
    })
    .await
}

#[tauri::command]
pub async fn chat_delete_category(
    state: tauri::State<'_, DispatcherState>,
    category_id: String,
) -> Result<(), String> {
    let db = state.db().clone();
    run_dispatcher_db("chat_delete_category", move || {
        db.delete_chat_category(&category_id)
    })
    .await
}

#[tauri::command]
pub async fn aha_get_chat_category_agent_configs(
    state: tauri::State<'_, DispatcherState>,
) -> Result<Vec<ChatCategoryAgentConfig>, String> {
    let db = state.db().clone();
    run_dispatcher_db("aha_get_chat_category_agent_configs", move || {
        db.list_chat_category_agent_configs()
    })
    .await
}

#[tauri::command]
pub async fn aha_save_chat_category_agent_configs(
    state: tauri::State<'_, DispatcherState>,
    configs: Vec<ChatCategoryAgentConfig>,
) -> Result<Vec<ChatCategoryAgentConfig>, String> {
    let db = state.db().clone();
    run_dispatcher_db("aha_save_chat_category_agent_configs", move || {
        db.save_chat_category_agent_configs(&configs)
    })
    .await
}

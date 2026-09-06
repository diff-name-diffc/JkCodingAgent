use super::*;

#[tauri::command]
pub async fn dispatcher_list_messages(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
) -> Result<Vec<DispatcherMessageRecord>, String> {
    let db = state.db().clone();
    run_dispatcher_db("dispatcher_list_messages", move || {
        db.list_visible_messages(&workspace_id)
    })
    .await
}

#[tauri::command]
pub async fn dispatcher_get_tool_run_tree(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
    tool_call_id: String,
    root_run_id: Option<String>,
) -> Result<Vec<DispatcherToolRunRecord>, String> {
    let db = state.db().clone();
    run_dispatcher_db("dispatcher_get_tool_run_tree", move || {
        db.list_tool_run_tree_for_call(&workspace_id, &tool_call_id, root_run_id.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn dispatcher_get_session_token_usage(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
) -> Result<Vec<DispatcherSessionTokenUsageRecord>, String> {
    let db = state.db().clone();
    run_dispatcher_db("dispatcher_get_session_token_usage", move || {
        db.list_session_token_usage(&workspace_id)
    })
    .await
}

#[tauri::command]
pub async fn dispatcher_clear_messages(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
) -> Result<(), String> {
    let db = state.db().clone();
    let workspace_for_cleanup = workspace_id.clone();
    let result = run_dispatcher_db("dispatcher_clear_messages", move || {
        db.clear_messages(&workspace_id)
    })
    .await;
    if result.is_ok() {
        // 会话资源清理规范：清空消息与会话删除同级回收内存态命令台账
        //（与 delete_session / project_delete 的执行对齐）。
        crate::agent::command_history::forget_session(&workspace_for_cleanup);
    }
    result
}

#[tauri::command]
pub async fn dispatcher_truncate_messages_from(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
    message_id: String,
) -> Result<u64, String> {
    let db = state.db().clone();
    run_dispatcher_db("dispatcher_truncate_messages_from", move || {
        db.truncate_messages_from(&workspace_id, &message_id)
    })
    .await
}

#[tauri::command]
pub async fn dispatcher_get_tool_artifact(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
    artifact_id: String,
) -> Result<DispatcherToolArtifactRecord, String> {
    let db = state.db().clone();
    run_dispatcher_db("dispatcher_get_tool_artifact", move || {
        db.get_tool_artifact(&workspace_id, &artifact_id)
    })
    .await
}

use super::*;

#[tauri::command]
pub async fn dispatcher_send_project_agent_message(
    state: tauri::State<'_, DispatcherState>,
    app: AppHandle,
    workspace_id: String,
    project_path: String,
    segments_json: String,
    on_event: Channel<AgentEvent>,
) -> Result<AgentTurn, String> {
    // G11-03：执行入口同样拒绝越权项目路径（canonicalize + 已注册工作区校验）。
    let validated_project_path = state.validate_project_workspace(&project_path).await?;
    let project_path = validated_project_path.to_string_lossy().into_owned();
    let title_segments_json = segments_json.clone();
    let agent = state.build_run_agent().await?.with_app_handle(app.clone());
    let run_handle = state.begin_run(&workspace_id).map_err(|e| e.to_string())?;
    let title_guard = state.begin_title_generation(&workspace_id);
    let keywords_guard = state.begin_keywords_generation(&workspace_id);
    let result = run_agent_turn(
        &agent,
        AgentRunRequest {
            kind: RuntimeAgentKind::Project,
            db: state.db(),
            workspace_id: &workspace_id,
            workspace_path: Some(&project_path),
            user_segments_json: segments_json,
            on_event,
            cancel_rx: run_handle.cancel_receiver(),
        },
    )
    .await
    .map_err(|error| error.to_string());
    // G11-09/10：运行槽位清理由句柄 RAII 负责（含 panic/提前 return 路径）。
    state.finish_run(run_handle);
    spawn_session_title_update(
        &state,
        &app,
        &workspace_id,
        &title_segments_json,
        AgentContext::Project,
        title_guard,
    );
    spawn_session_keywords_update(
        &state,
        &app,
        &workspace_id,
        AgentContext::Project,
        keywords_guard,
    );
    result
}

#[tauri::command]
pub async fn dispatcher_send_chat_agent_message(
    state: tauri::State<'_, DispatcherState>,
    app: AppHandle,
    workspace_id: String,
    segments_json: String,
    on_event: Channel<AgentEvent>,
) -> Result<AgentTurn, String> {
    let title_segments_json = segments_json.clone();
    let agent = state
        .build_plain_chat_agent(&workspace_id)
        .await?
        .with_app_handle(app.clone());
    let run_handle = state.begin_run(&workspace_id).map_err(|e| e.to_string())?;
    let title_guard = state.begin_title_generation(&workspace_id);
    let keywords_guard = state.begin_keywords_generation(&workspace_id);
    let result = run_agent_turn(
        &agent,
        AgentRunRequest {
            kind: RuntimeAgentKind::PlainChat,
            db: state.db(),
            workspace_id: &workspace_id,
            workspace_path: None,
            user_segments_json: segments_json,
            on_event,
            cancel_rx: run_handle.cancel_receiver(),
        },
    )
    .await
    .map_err(|error| error.to_string());
    // G11-09/10：运行槽位清理由句柄 RAII 负责（含 panic/提前 return 路径）。
    state.finish_run(run_handle);
    spawn_session_title_update(
        &state,
        &app,
        &workspace_id,
        &title_segments_json,
        AgentContext::Chat,
        title_guard,
    );
    spawn_session_keywords_update(
        &state,
        &app,
        &workspace_id,
        AgentContext::Chat,
        keywords_guard,
    );
    result
}

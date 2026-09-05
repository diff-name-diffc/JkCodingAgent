use super::*;

/// 架构设计视觉 Agent 的消息入口。
///
/// 镜像 `dispatcher_send_chat_agent_message`；差异：
/// - Agent 经 `build_architecture_agent` 构建——按 `model_library_id` 解析
///   视觉模型库条目，缺省回退设置中视觉用途绑定；
/// - **跳过会话关键字生成**：架构会话不出现在全局会话搜索（隔离面收敛），
///   标题生成保留（会话列表仍需要可读标题）。
#[tauri::command]
pub async fn dispatcher_send_architecture_agent_message(
    state: tauri::State<'_, DispatcherState>,
    app: AppHandle,
    workspace_id: String,
    segments_json: String,
    model_library_id: Option<String>,
    on_event: Channel<AgentEvent>,
) -> Result<AgentTurn, String> {
    let title_segments_json = segments_json.clone();
    let agent = state
        .build_architecture_agent(model_library_id.as_deref())
        .await?
        .with_app_handle(app.clone());
    let run_handle = state.begin_run(&workspace_id).map_err(|e| e.to_string())?;
    let title_guard = state.begin_title_generation(&workspace_id);
    let result = run_agent_turn(
        &agent,
        AgentRunRequest {
            kind: RuntimeAgentKind::Architecture,
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
    result
}

/// 回传架构画布程序的执行报告。前端画布解释器执行完毕（或画布未就绪）后
/// 调用，解除 `architecture_run` 工具的等待。报告已被消费返回 true；
/// 槽位已因超时/取消清槽或重复回传返回 false（无副作用，前端无需处理）。
#[tauri::command]
pub async fn architecture_run_complete(
    state: tauri::State<'_, DispatcherState>,
    run_id: String,
    report: String,
) -> Result<bool, String> {
    Ok(state.complete_arch_run(&run_id, report))
}

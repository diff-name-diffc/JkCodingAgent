use super::*;

/// 三个 send 命令的公共执行骨架（原三处 35-40 行的同序列收敛为一份）：
/// begin_run 占槽 → 元数据守卫 → `run_agent_turn` → finish → spawn 标题/
/// 关键字生成。命令层只剩差异声明（agent 构建 / kind / workspace_path /
/// 是否生成关键字），Agent 构建（含 `with_app_handle`）由 `agent_future`
/// 惰性完成——三类 Agent 的构建链互不相同，不宜在骨架内特判。
pub(crate) async fn run_agent_turn_skeleton<A, F>(
    state: &tauri::State<'_, DispatcherState>,
    app: &AppHandle,
    workspace_id: &str,
    segments_json: String,
    on_event: Channel<AgentEvent>,
    kind: RuntimeAgentKind,
    workspace_path: Option<String>,
    agent_future: F,
    with_keywords: bool,
) -> Result<AgentTurn, String>
where
    A: AgentRunAdapter,
    F: std::future::Future<Output = Result<A, String>>,
{
    let agent = agent_future.await?;
    let title_segments_json = segments_json.clone();
    let run_handle = state.begin_run(workspace_id).map_err(|e| e.to_string())?;
    let title_guard = state.begin_title_generation(workspace_id);
    let keywords_guard = if with_keywords {
        Some(state.begin_keywords_generation(workspace_id))
    } else {
        None
    };
    let result = run_agent_turn(
        &agent,
        AgentRunRequest {
            kind,
            db: state.db(),
            workspace_id,
            workspace_path: workspace_path.as_deref(),
            user_segments_json: segments_json,
            on_event,
            cancel_rx: run_handle.cancel_receiver(),
        },
    )
    .await
    .map_err(|error| error.to_string());
    // G11-09/10：运行槽位清理由句柄 RAII 负责（含 panic/提前 return 路径）。
    state.finish_run(run_handle);
    let agent_context = match kind {
        RuntimeAgentKind::Project => AgentContext::Project,
        _ => AgentContext::Chat,
    };
    spawn_session_title_update(
        state,
        app,
        workspace_id,
        &title_segments_json,
        agent_context,
        title_guard,
    );
    if let Some(keywords_guard) = keywords_guard {
        spawn_session_keywords_update(state, app, workspace_id, agent_context, keywords_guard);
    }
    result
}

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
    let agent_app = app.clone();
    run_agent_turn_skeleton(
        &state,
        &app,
        &workspace_id,
        segments_json,
        on_event,
        RuntimeAgentKind::Project,
        Some(project_path),
        async {
            state
                .build_run_agent()
                .await
                .map(|agent| agent.with_app_handle(agent_app))
        },
        true,
    )
    .await
}

#[tauri::command]
pub async fn dispatcher_send_chat_agent_message(
    state: tauri::State<'_, DispatcherState>,
    app: AppHandle,
    workspace_id: String,
    segments_json: String,
    on_event: Channel<AgentEvent>,
) -> Result<AgentTurn, String> {
    let agent_app = app.clone();
    // workspace_id 同时被骨架参数借用与 agent 构建闭包持有，克隆一份给后者。
    let agent_workspace = workspace_id.clone();
    run_agent_turn_skeleton(
        &state,
        &app,
        &workspace_id,
        segments_json,
        on_event,
        RuntimeAgentKind::PlainChat,
        None,
        async {
            state
                .build_plain_chat_agent(&agent_workspace)
                .await
                .map(|agent| agent.with_app_handle(agent_app))
        },
        true,
    )
    .await
}

/// 请求停止会话当前运行：向活动 run 的 watch channel 发取消信号；随后
/// 无论结果关闭该会话浏览器（run 可能驱动浏览器动作，停 run 必须连带
/// 停浏览器会话，避免残留有头窗口）。
///
/// 它是唯一额外注入 BrowserManager State 的 agent 命令——正因这条
/// 「停 run 联动停浏览器」的跨域副作用。归组 run_commands（运行入口
/// 与停止对偶），此前误置于 settings_commands。
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

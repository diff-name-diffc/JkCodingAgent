use super::*;
use super::run_commands::run_agent_turn_skeleton;

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
    let agent_app = app.clone();
    run_agent_turn_skeleton(
        &state,
        &app,
        &workspace_id,
        segments_json,
        on_event,
        RuntimeAgentKind::Architecture,
        None,
        async {
            state
                .build_architecture_agent(model_library_id.as_deref())
                .await
                .map(|agent| agent.with_app_handle(agent_app))
        },
        false,
    )
    .await
}

/// 回传架构画布程序的执行报告。前端画布解释器执行完毕（或画布未就绪）后
/// 调用，解除 `architecture_run` 工具的等待。报告已被消费返回 true；
/// 槽位已因超时/取消清槽、重复回传或 workspace 不匹配返回 false（无副作用，
/// 前端无需处理）。workspace 校验与 `dispatcher_get_tool_artifact` 等命令的
/// 域校验风格对齐。
#[tauri::command]
pub async fn architecture_run_complete(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
    run_id: String,
    report: String,
) -> Result<bool, String> {
    Ok(state.complete_arch_run(&run_id, &workspace_id, report))
}

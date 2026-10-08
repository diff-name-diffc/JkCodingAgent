use tauri::State;

use crate::agent::state::DispatcherState;

/// 回传命令审查「需用户确认」弹窗的用户裁决，解除阻塞中的命令类工具。
///
/// 工具侧在审查不通过时登记一次性请求并 emit `tool-confirm-request`；用户在前端
/// 弹窗点「允许执行 / 拒绝」后由本命令回传 `approved`。裁决已被消费返回 true；
/// 槽位已因超时/取消清槽、重复回传或 workspace 不匹配返回 false（无副作用，
/// 前端无需处理）。workspace 校验与 `architecture_run_complete` 等命令同风格。
#[tauri::command]
pub async fn tool_confirm_resolve(
    state: State<'_, DispatcherState>,
    workspace_id: String,
    request_id: String,
    approved: bool,
) -> Result<bool, String> {
    Ok(state.resolve_user_confirm(&request_id, &workspace_id, approved))
}

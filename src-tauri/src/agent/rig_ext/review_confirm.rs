//! 命令审查「需用户确认」的通用门禁：审查不通过但用户可人工放行的统一通道。
//!
//! 与「被审查直接阻断」的区别：当审查结论不是硬拒绝、而是需要用户知情后再决定
//! 时（审查原因含 [`USER_CONFIRM_MARKER`]，或提权命令被拦截），工具不再直接把
//! 命令丢掉，而是登记一次性请求 + emit `tool-confirm-request`，阻塞等待前端弹窗
//! 的用户裁决：允许则放行执行，拒绝/超时/取消则按 fail-closed 走原拦截。
//!
//! 复用 [`ArchRunRegistry`](crate::agent::state) 同款的「事件出、命令按 id 回」
//! 往返桥（前端经 `tool_confirm_resolve` 回传），因此本模块只依赖
//! `DispatcherState` 与 `AppHandle`，命令类工具（ssh_exec / local_zsh /
//! sync_directory / MCP / 通用审查）共用同一入口。

use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::watch;

use crate::agent::ssh_review::USER_CONFIRM_MARKER;
use crate::agent::state::DispatcherState;
use crate::shared::cancel::wait_for_cancel;

/// 等待用户弹窗裁决的时限（秒）。低于 ssh_exec 自管工具的 settle ceiling（600s），
/// 保证即便用户长时间不响应，工具也在策略层兜底上限之前按超时 fail-closed 收敛。
const CONFIRM_TIMEOUT_SECS: u64 = 180;

/// 前端弹窗展示所需的确认请求（脱敏：只含命令本身与审查原因，不含任何凭据）。
#[derive(Debug, Clone)]
pub struct ConfirmRequest {
    /// 发起调用的会话（用于回传侧的 workspace 域校验与前端定位）。
    pub workspace_id: String,
    /// 工具名（ssh_exec / local_zsh / sync_directory / MCP 工具名 / 通用工具名）。
    pub tool: String,
    /// 目标环境的简短人类可读标签（服务器 id+登录用户 / 本地 zsh / 工作区等）。
    pub target: String,
    /// 待执行的命令全文（原样，未包装）。
    pub command: String,
    /// 审查给出的拦截原因。
    pub reason: String,
    /// 是否提权（sudo）命令——提权被拦截时一律弹窗，并在界面显著提示。
    pub elevated: bool,
}

/// 用户裁决的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmOutcome {
    /// 用户明确允许执行。
    Approved,
    /// 用户明确拒绝（或命令本身无人可确认）。
    Denied,
    /// 等待期间命令被取消。
    Cancelled,
    /// 等待期间超过时限无人裁决（fail-closed）。
    Timeout,
}

/// 送前端的确认请求载荷（camelCase，`requestId` 作为回传凭据）。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolConfirmRequestPayload {
    request_id: String,
    workspace_id: String,
    session_id: String,
    tool: String,
    target: String,
    command: String,
    reason: String,
    elevated: bool,
}

/// 是否应把「审查不通过」升级为「弹窗请用户确认」：
/// - 提权命令被拦截：一律弹窗（用户对该场景有明确的知情要求）；
/// - 其余：仅当审查原因带 [`USER_CONFIRM_MARKER`]（需用户确认）标记——硬拒绝
///   （如 rm -rf 根目录）不弹窗，保持直接阻断。
pub fn needs_user_confirmation(reason: &str, elevated: bool) -> bool {
    elevated || reason.contains(USER_CONFIRM_MARKER)
}

/// 请求用户人工确认一条被审查拦截的命令，阻塞等待裁决。
///
/// - 无 `AppHandle`（脱离 UI 的调用，如子智能体/工作流节点）：直接 [`ConfirmOutcome::Denied`]，
///   维持 fail-closed。
/// - 登记成功后发 `tool-confirm-request`；`select!(biased)` 优先级为
///   裁决就绪 > 取消 > 超时。超时/取消一律调 `remove_user_confirm` 清槽——
///   此后迟到的 `tool_confirm_resolve` 找不到条目、无副作用。
pub async fn request_confirmation(
    app: Option<&AppHandle>,
    cancel_rx: Option<watch::Receiver<bool>>,
    request: ConfirmRequest,
) -> ConfirmOutcome {
    let Some(app) = app else {
        // 无 UI 通道：无法确认，按拒绝处理（fail-closed）。
        return ConfirmOutcome::Denied;
    };
    let state = app.state::<DispatcherState>();
    let (request_id, mut resolve_rx) = state.begin_user_confirm(&request.workspace_id);
    let payload = ToolConfirmRequestPayload {
        request_id: request_id.clone(),
        workspace_id: request.workspace_id.clone(),
        session_id: request.workspace_id.clone(),
        tool: request.tool.clone(),
        target: request.target.clone(),
        command: request.command.clone(),
        reason: request.reason.clone(),
        elevated: request.elevated,
    };
    if let Err(error) = app.emit("tool-confirm-request", payload) {
        // 通道不可用：清槽并 fail-closed，不把命令放行。
        state.remove_user_confirm(&request_id);
        eprintln!("[tool-confirm] 发送确认请求失败：{error}");
        return ConfirmOutcome::Denied;
    }

    let outcome = tokio::select! {
        biased;
        received = &mut resolve_rx => match received {
            Ok(true) => ConfirmOutcome::Approved,
            Ok(false) => ConfirmOutcome::Denied,
            Err(_) => ConfirmOutcome::Denied,
        },
        _ = wait_for_cancel(cancel_rx) => ConfirmOutcome::Cancelled,
        _ = tokio::time::sleep(Duration::from_secs(CONFIRM_TIMEOUT_SECS)) => ConfirmOutcome::Timeout,
    };

    match outcome {
        ConfirmOutcome::Approved => ConfirmOutcome::Approved,
        other => {
            // 未获批准（拒绝/超时/取消）：清槽，让迟到裁决无副作用。
            state.remove_user_confirm(&request_id);
            other
        }
    }
}

/// 取消等待的终态是否视为「拒绝」（当前 Cancelled 与 Timeout 均归拒绝语义）。
#[cfg(test)]
fn cancelled_or_timeout(outcome: ConfirmOutcome) -> bool {
    matches!(outcome, ConfirmOutcome::Cancelled | ConfirmOutcome::Timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elevation_always_requires_confirmation() {
        assert!(needs_user_confirmation("提权后删除系统文件", true));
        assert!(needs_user_confirmation("", true));
    }

    #[test]
    fn marker_reason_requires_confirmation() {
        let reason = format!("「{USER_CONFIRM_MARKER}」涉及非本任务进程");
        assert!(needs_user_confirmation(&reason, false));
    }

    #[test]
    fn hard_denial_without_marker_does_not_ask() {
        assert!(!needs_user_confirmation(
            "rm -rf 指向根目录，删除系统文件",
            false
        ));
    }

    #[test]
    fn cancel_wait_outcome_classification_is_rejection() {
        assert!(cancelled_or_timeout(ConfirmOutcome::Cancelled));
        assert!(cancelled_or_timeout(ConfirmOutcome::Timeout));
        assert!(!cancelled_or_timeout(ConfirmOutcome::Approved));
        assert!(!cancelled_or_timeout(ConfirmOutcome::Denied));
    }
}

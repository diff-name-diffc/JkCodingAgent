use super::{classify_ssh_result, map_command_failure};
use crate::ssh_tool::{CommandFailure, CommandFailureKind, SshExecResult};
use rig::tool::ToolErrorKind;

fn result(exit_code: i32) -> SshExecResult {
    SshExecResult {
        server_id: "server".into(),
        session_id: "session".into(),
        exit_code,
        stdout: "captured output".into(),
        stderr: String::new(),
        duration_ms: 1,
        truncated: false,
        interactive_blocked: false,
        cancelled: false,
        external_state_unknown: false,
    }
}

#[test]
fn captured_nonzero_exit_is_failure_and_preserves_exit_code() {
    let result = result(7);
    let payload = serde_json::to_string(&result).unwrap();
    let error = classify_ssh_result(&result, payload.clone()).unwrap_err();
    assert_eq!(error.code(), Some("command_failed"));
    assert_eq!(error.message(), payload);
}

#[test]
fn unknown_remote_state_and_cancellation_are_not_success() {
    let mut result = result(0);
    result.external_state_unknown = true;
    let error = classify_ssh_result(&result, "unknown".into()).unwrap_err();
    assert_eq!(error.code(), Some("external_state_unknown"));
    result.cancelled = true;
    let error = classify_ssh_result(&result, "cancelled".into()).unwrap_err();
    assert_eq!(error.code(), Some("external_state_unknown"));
}

#[test]
fn interactive_blocked_is_command_failed_not_unknown_state() {
    // command_exec 对交互阻塞同时置 external_state_unknown=true；
    // 分类须优先归入 command_failed（引导改写为非交互命令后重试），
    // 而非 external_state_unknown（禁止自动重跑）。
    let mut result = result(-1);
    result.interactive_blocked = true;
    result.external_state_unknown = true;
    let error = classify_ssh_result(&result, "blocked".into()).unwrap_err();
    assert_eq!(error.code(), Some("command_failed"));
    assert_eq!(error.message(), "blocked");
}

#[test]
fn pre_send_cancellation_maps_to_cancelled() {
    let failure = CommandFailure {
        message: "SSH 调用已取消，未发送命令".into(),
        stale: false,
        kind: CommandFailureKind::CancelledNotSent,
    };
    let error = map_command_failure(failure);
    assert_eq!(error.kind(), ToolErrorKind::Cancelled);
    assert_eq!(error.message(), "错误：SSH 调用已取消，未发送命令");
}

#[test]
fn external_state_unknown_failure_keeps_code_and_is_not_retryable() {
    let failure = CommandFailure {
        message: "远程执行请求失败，结果未知，禁止自动重跑：boom".into(),
        stale: false,
        kind: CommandFailureKind::ExternalStateUnknown,
    };
    let error = map_command_failure(failure);
    assert_eq!(error.kind(), ToolErrorKind::Other);
    assert_eq!(error.code(), Some("external_state_unknown"));
    assert_eq!(error.retryable(), Some(false));
    assert_eq!(
        error.message(),
        "错误：SSH 命令执行失败：远程执行请求失败，结果未知，禁止自动重跑：boom"
    );
}

#[test]
fn other_failure_maps_to_plain_error_without_code() {
    let failure = CommandFailure::other("创建 SSH channel 失败：boom".into());
    let error = map_command_failure(failure);
    assert_eq!(error.kind(), ToolErrorKind::Other);
    assert_eq!(error.code(), None);
    assert_eq!(
        error.message(),
        "错误：SSH 命令执行失败：创建 SSH channel 失败：boom"
    );
}

#[test]
fn successful_capture_preserves_output() {
    assert_eq!(
        classify_ssh_result(&result(0), "output".into()).unwrap(),
        "output"
    );
}

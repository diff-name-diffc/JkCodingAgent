use super::*;

#[cfg(unix)]
#[test]
fn nonzero_exit_and_timeout_are_structured_failures() {
    use std::os::unix::process::ExitStatusExt;
    let mut captured = CapturedCommandOutput {
        output: Output {
            status: std::process::ExitStatus::from_raw(7 << 8),
            stdout: vec![],
            stderr: vec![],
        },
        total_bytes_read: 0,
        timed_out: false,
        cancelled: false,
    };
    let error = classify_command_result(&captured, "exit=7".into()).unwrap_err();
    assert_eq!(error.code(), Some("command_failed"));
    assert_eq!(error.message(), "exit=7");
    captured.timed_out = true;
    assert_eq!(
        classify_command_result(&captured, "timeout".into())
            .unwrap_err()
            .retryable(),
        Some(false)
    );
}

#[tokio::test]
async fn invalid_and_blacklisted_commands_fail_before_execution() {
    for args in [json!({}), json!({"command": "cd /"})] {
        let result = run_local_zsh(
            &args,
            std::env::temp_dir(),
            "test".into(),
            30,
            None,
            crate::agent::rig_ext::review::RigReviewContext::unconfigured(),
        )
        .await;
        assert!(result.is_err());
    }
}

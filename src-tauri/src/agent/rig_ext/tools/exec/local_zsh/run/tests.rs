use super::*;

fn finished(exit_code: Option<i32>, timed_out: bool, cancelled: bool) -> sessions::FinishedRun {
    sessions::FinishedRun {
        exit_code,
        timed_out,
        cancelled,
        duration_ms: 10,
        stdout: String::new(),
        stderr: String::new(),
        output_truncated: false,
        history: Vec::new(),
        audit_error: None,
    }
}

#[test]
fn nonzero_exit_and_timeout_are_structured_failures() {
    let error = classify_finished(&finished(Some(7), false, false), "exit=7".into()).unwrap_err();
    assert_eq!(error.code(), Some("command_failed"));
    assert_eq!(error.message(), "exit=7");

    let timeout = classify_finished(&finished(Some(9), true, false), "timeout".into()).unwrap_err();
    assert_eq!(timeout.retryable(), Some(false));

    let cancelled =
        classify_finished(&finished(None, false, true), "cancelled".into()).unwrap_err();
    assert_eq!(cancelled.kind(), rig::tool::ToolErrorKind::Cancelled);
}

#[test]
fn audit_write_failure_keeps_cancellation_priority() {
    let mut run = finished(Some(0), false, false);
    run.audit_error = Some("disk full".into());
    let error = classify_finished(&run, "ok".into()).unwrap_err();
    assert_eq!(error.code(), Some("audit_failed"));
    assert_eq!(error.retryable(), Some(false));

    run.cancelled = true;
    let cancelled = classify_finished(&run, "ok".into()).unwrap_err();
    assert_eq!(cancelled.kind(), rig::tool::ToolErrorKind::Cancelled);
}

#[tokio::test]
async fn invalid_and_blacklisted_commands_fail_before_execution() {
    for args in [
        json!({}),
        json!({ "command": "" }),
        json!({ "command": "cd /" }),
    ] {
        let result = run_local_zsh(
            &args,
            std::env::temp_dir(),
            None,
            "test".into(),
            30,
            None,
            crate::agent::rig_ext::review::RigReviewContext::unconfigured(),
            None,
        )
        .await;
        assert!(result.is_err(), "args={args}");
    }
}

#[tokio::test]
async fn wait_run_id_rejects_unknown_ids_and_command_combination() {
    for args in [
        json!({ "wait_run_id": "zsh-nope" }),
        json!({ "wait_run_id": "zsh-nope", "command": "echo hi" }),
    ] {
        let result = run_local_zsh(
            &args,
            std::env::temp_dir(),
            None,
            "wait-args-test".into(),
            30,
            None,
            crate::agent::rig_ext::review::RigReviewContext::unconfigured(),
            None,
        )
        .await;
        assert!(result.is_err(), "args={args}");
    }
}

#[tokio::test]
async fn blocked_project_command_audits_only_in_its_session_directory() {
    let guard = crate::test_util::TempDirGuard::new("local-zsh-audit-owner");
    let base = guard.path().canonicalize().unwrap();
    let session_root = base.join("session");
    let project = base.join("project");
    for path in [&session_root, &project] {
        std::fs::create_dir_all(path).unwrap();
    }
    let result = run_local_zsh(
        &json!({ "command": "touch should-not-exist", "working_directory": project }),
        session_root.clone(),
        Some(ChatWorkspace {
            session_root: session_root.clone(),
            project_roots: vec![project.clone()],
        }),
        "audit-owner-test".into(),
        30,
        None,
        crate::agent::rig_ext::review::RigReviewContext::unconfigured(),
        None,
    )
    .await;
    assert!(result.is_err());
    assert!(!project.join("should-not-exist").exists());
    assert!(!project.join(AUDIT_FILE_NAME).exists());
    let audit_path = local_zsh_dir(&session_root).unwrap().join(AUDIT_FILE_NAME);
    let audit: LocalZshAuditLog =
        serde_json::from_slice(&std::fs::read(audit_path).unwrap()).unwrap();
    assert_eq!(
        audit.entries[0].working_directory.as_deref(),
        project.to_str()
    );
}

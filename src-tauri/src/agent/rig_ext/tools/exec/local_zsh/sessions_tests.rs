//! 监督器行为测试：正常退出、yield 转后台、取消按进程组终止、tty 模式、
//! 注册表会话隔离与 cwd/TMPDIR 传递。

use super::*;
use crate::test_util::TempDirGuard;

fn context(guard: &TempDirGuard, workspace_id: &str, timeout_secs: u64) -> SupervisorContext {
    let run_dir = guard.path().join("run");
    std::fs::create_dir_all(&run_dir).unwrap();
    SupervisorContext {
        workspace_id: workspace_id.to_string(),
        command: String::new(),
        run_dir,
        audit_dir: guard.path().to_path_buf(),
        temp_dir: Some(guard.path().join("tmp")),
        review: crate::ssh_tool::SshAuditReview {
            allowed: true,
            user_approved: false,
            reason: "测试放行".to_string(),
        },
        exec_timeout_secs: timeout_secs,
        cancel_rx: None,
        history_with_cwd: false,
    }
}

#[tokio::test]
async fn fast_command_finishes_within_yield_window() {
    let guard = TempDirGuard::new("zsh-fast");
    let mut ctx = context(&guard, "fast-test", 30);
    ctx.command = "print -r -- hello-supervisor".to_string();
    let run = start_command(ctx, false).unwrap();
    let snapshot = wait_for(&run, std::time::Duration::from_secs(10)).await;
    let RunSnapshot::Finished(finished) = snapshot else {
        panic!("快命令应在窗口内结束");
    };
    assert_eq!(finished.exit_code, Some(0));
    assert!(
        finished.stdout.contains("hello-supervisor"),
        "{:?}",
        finished.stdout
    );
    assert!(finished.audit_error.is_none(), "{:?}", finished.audit_error);
    reap(&run.run_id, "fast-test");
}

#[tokio::test]
async fn slow_command_yields_then_finishes_on_second_wait() {
    let guard = TempDirGuard::new("zsh-yield");
    let mut ctx = context(&guard, "yield-test", 30);
    ctx.command = "sleep 0.5; print -r -- late-output".to_string();
    let run = start_command(ctx, false).unwrap();

    // 短窗口：转后台。
    let RunSnapshot::Running { run_id, stdout, .. } =
        wait_for(&run, std::time::Duration::from_millis(50)).await
    else {
        panic!("sleep 命令应在 50ms 窗口后仍在运行");
    };
    assert_eq!(run_id, run.run_id);
    assert!(stdout.is_empty());

    // 收割路径：lookup → wait → Finished。
    let waited = lookup(&run_id, "yield-test").expect("注册表应有该运行");
    let RunSnapshot::Finished(finished) =
        wait_for(&waited, std::time::Duration::from_secs(10)).await
    else {
        panic!("第二次等待应等到结束");
    };
    assert_eq!(finished.exit_code, Some(0));
    assert!(finished.stdout.contains("late-output"));
    reap(&run_id, "yield-test");
    assert!(
        lookup(&run_id, "yield-test").is_none(),
        "收割后应从注册表移除"
    );
}

#[tokio::test]
async fn cancellation_kills_the_process_group() {
    let guard = TempDirGuard::new("zsh-cancel");
    let mut ctx = context(&guard, "cancel-test", 30);
    ctx.command = "sleep 30 & wait".to_string();
    let (cancel_tx, cancel_rx) = watch::channel(false);
    ctx.cancel_rx = Some(cancel_rx);
    cancel_tx.send(true).expect("request cancellation");

    let run = start_command(ctx, false).unwrap();
    let RunSnapshot::Finished(finished) = wait_for(&run, std::time::Duration::from_secs(5)).await
    else {
        panic!("预取消命令应立即以取消收口");
    };
    assert!(finished.cancelled);
    assert!(!finished.timed_out);
    reap(&run.run_id, "cancel-test");
}

#[tokio::test]
async fn hard_timeout_terminates_and_reports() {
    let guard = TempDirGuard::new("zsh-timeout");
    let mut ctx = context(&guard, "timeout-test", 1);
    ctx.command = "sleep 30".to_string();
    let run = start_command(ctx, false).unwrap();
    let RunSnapshot::Finished(finished) = wait_for(&run, std::time::Duration::from_secs(5)).await
    else {
        panic!("硬超时应在窗口内触发");
    };
    assert!(finished.timed_out);
    assert!(!finished.cancelled);
    reap(&run.run_id, "timeout-test");
}

#[tokio::test]
async fn tty_mode_merges_output_and_exits_cleanly() {
    let guard = TempDirGuard::new("zsh-tty");
    let mut ctx = context(&guard, "tty-test", 30);
    ctx.command = "print -r -- tty-hello; print -r -- tty-stderr 1>&2".to_string();
    let run = start_command(ctx, true).unwrap();
    let RunSnapshot::Finished(finished) = wait_for(&run, std::time::Duration::from_secs(10)).await
    else {
        panic!("tty 快命令应在窗口内结束");
    };
    assert_eq!(finished.exit_code, Some(0));
    assert!(
        finished.stdout.contains("tty-hello") && finished.stdout.contains("tty-stderr"),
        "PTY 输出应合并到 stdout：{:?}",
        finished.stdout
    );
    reap(&run.run_id, "tty-test");
}

#[tokio::test]
async fn registry_scopes_runs_to_their_session() {
    let guard = TempDirGuard::new("zsh-scope");
    let mut ctx = context(&guard, "session-a", 30);
    ctx.command = "sleep 1".to_string();
    let run = start_command(ctx, false).unwrap();
    assert!(lookup(&run.run_id, "session-a").is_some());
    assert!(
        lookup(&run.run_id, "session-b").is_none(),
        "跨会话探测必须按不存在处理"
    );
    reap(&run.run_id, "session-a");
}

#[tokio::test]
async fn selected_cwd_and_tmpdir_reach_the_child_process() {
    let guard = TempDirGuard::new("zsh-environment");
    let base = guard.path().canonicalize().unwrap();
    let project = base.join("project");
    let temp = base.join("session-tmp");
    for path in [&project, &temp] {
        std::fs::create_dir_all(path).unwrap();
    }
    let ctx = SupervisorContext {
        workspace_id: "env-test".to_string(),
        command: "pwd; print -r -- \"$TMPDIR\"".to_string(),
        run_dir: project.clone(),
        audit_dir: base.clone(),
        temp_dir: Some(temp.clone()),
        review: crate::ssh_tool::SshAuditReview {
            allowed: true,
            user_approved: false,
            reason: String::new(),
        },
        exec_timeout_secs: 30,
        cancel_rx: None,
        history_with_cwd: false,
    };
    let run = start_command(ctx, false).unwrap();
    let RunSnapshot::Finished(finished) = wait_for(&run, std::time::Duration::from_secs(10)).await
    else {
        panic!("环境探测命令应在窗口内结束");
    };
    assert_eq!(finished.exit_code, Some(0));
    let lines: Vec<&str> = finished.stdout.lines().collect();
    assert_eq!(
        lines,
        vec![project.to_str().unwrap(), temp.to_str().unwrap()],
        "cwd 与 TMPDIR 应传递给子进程：{:?}",
        finished.stdout
    );
    reap(&run.run_id, "env-test");
}

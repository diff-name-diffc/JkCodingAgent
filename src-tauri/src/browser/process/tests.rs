use super::*;
use crate::browser::tests::fake_process;
use std::sync::atomic::AtomicUsize;

#[tokio::test]
async fn preview_frames_are_scoped_to_active_run_and_process_stream() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let collected = seen.clone();
    let (_fixture, process) = fake_process(Arc::new(move |name, value| {
        if name == "browser-frame" {
            collected.lock().push(value);
        }
    }));
    let frame = |run: Option<&str>, tool: Option<&str>, sequence: u64| {
        json!({
            "event":"frame","sessionId":"session","runId":run,"toolCallId":tool,
            "sequence":sequence,"capturedAt":1_700_000_000_000u64,"url":"about:blank",
            "data":"data:image/jpeg;base64,AA==","width":1280,"height":800
        })
    };
    process.handle_message(frame(None, None, 1));
    process.set_agent("run1".into(), "tool1".into(), Arc::new(|_| {}));
    process.handle_message(frame(None, None, 2));
    process.handle_message(frame(Some("run1"), Some("wrong"), 3));
    process.handle_message(frame(Some("run1"), Some("tool1"), 4));
    process.handle_message(frame(Some("run1"), Some("tool1"), 4));
    process.set_agent("run2".into(), "tool2".into(), Arc::new(|_| {}));
    process.handle_message(frame(Some("run1"), Some("tool1"), 5));
    process.finish_agent();
    process.handle_message(frame(Some("run2"), Some("tool2"), 6));
    process.handle_message(frame(None, None, 7));
    let frames = seen.lock().clone();
    assert_eq!(
        frames
            .iter()
            .map(|frame| frame["sequence"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![1, 4, 7]
    );
    assert!(frames
        .iter()
        .all(|frame| frame["streamId"] == process.stream_id));
    process.terminate("测试回收").await.unwrap();
    let (_replacement_fixture, replacement) = fake_process(Arc::new(|_, _| {}));
    assert_ne!(process.stream_id, replacement.stream_id);
    replacement.terminate("测试回收").await.unwrap();
}

#[tokio::test]
async fn preview_errors_do_not_reach_agent_events_or_stop_execution() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let collected = seen.clone();
    let (_fixture, process) = fake_process(Arc::new(move |name, value| {
        if name == "browser-preview-error" {
            collected.lock().push(value);
        }
    }));
    let agent_events = Arc::new(AtomicUsize::new(0));
    let observed = agent_events.clone();
    process.set_agent(
        "run".into(),
        "tool".into(),
        Arc::new(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
        }),
    );
    process.handle_message(json!({"event":"preview_error","sessionId":"session","runId":"old","toolCallId":"tool","message":"stale"}));
    process.handle_message(json!({"event":"preview_error","sessionId":"session","runId":"run","toolCallId":"tool","message":"capture failed"}));
    process.report_preview_error("IPC timeout".into());
    assert_eq!(seen.lock().len(), 2);
    assert_eq!(seen.lock()[0]["toolCallId"], "tool");
    assert_eq!(agent_events.load(Ordering::SeqCst), 0);
    assert!(process.is_alive());
    process.request("start", json!({})).await.unwrap();
    process.terminate("测试回收").await.unwrap();
}

#[tokio::test]
async fn background_termination_waits_for_executor_completion() {
    let (_fixture, process) = fake_process(Arc::new(|_, _| {}));
    let (started, running) = oneshot::channel();
    let (release, finish) = std::sync::mpsc::channel();
    process.abort_in_background("测试后台终止", move |pid| {
        terminate_process_group(pid)?;
        started.send(()).unwrap();
        finish
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        Ok(())
    });
    running.await.unwrap();
    assert!(!*process.termination_done.borrow());
    assert_eq!(process.status().state, "stopping");
    let result = process.terminate("测试回收");
    tokio::pin!(result);
    assert!(futures::poll!(&mut result).is_pending());
    release.send(()).unwrap();
    result.await.unwrap();
    assert!(process.has_exited());
    assert_eq!(process.status().state, "closed");
}

#[tokio::test]
async fn background_termination_error_cannot_confirm_process_tree_exit() {
    let (_fixture, process) = fake_process(Arc::new(|_, _| {}));
    process.abort_in_background("测试后台终止", |pid| {
        terminate_process_group(pid)?;
        Err("injected taskkill exit code 1".into())
    });
    let error = process
        .terminate_with("测试回收", |_| {
            std::future::ready(Err("injected taskkill exit code 1".into()))
        })
        .await
        .unwrap_err();
    assert!(error.contains("exit code 1"));
    assert!(*process.termination_done.borrow());
    assert!(matches!(&*process.exited.borrow(), Some(Ok(()))));
    assert!(!process.has_exited());
    assert_eq!(process.status().state, "error");
    process.terminate("测试最终回收").await.unwrap();
}

#[test]
fn termination_command_checks_exit_status_and_bounds_timeout() {
    use std::time::Duration;
    let fixture = crate::test_util::TempDirGuard::new("browser-termination-command");
    let marker = fixture.path().join("late-effect");
    let command = |source: &str| {
        let mut command = std::process::Command::new("python3");
        command.args(["-c", source]);
        command.arg(&marker);
        command
    };
    termination::run_command(command("pass"), Duration::from_secs(2)).unwrap();
    let failure =
        termination::run_command(command("import sys; sys.exit(17)"), Duration::from_secs(2))
            .unwrap_err();
    assert!(failure.contains("17"));
    let timeout = termination::run_command(
        command(
            "import pathlib,sys,time; time.sleep(.4); pathlib.Path(sys.argv[1]).write_text('late')",
        ),
        Duration::from_millis(100),
    )
    .unwrap_err();
    assert!(timeout.contains("超时"));
    std::thread::sleep(Duration::from_millis(500));
    assert!(!marker.exists(), "超时命令必须被回收，不能继续执行");
}

#[tokio::test]
async fn transient_signal_failure_waits_for_exit_before_rechecking_process_group() {
    let (_fixture, process) = fake_process(Arc::new(|_, _| {}));
    // 确定性注入首次 killpg 失败；此时服务仍存活，退出通知尚未发布。
    process.abort_with("测试首次终止失败", |_| Err("injected EPERM".into()));
    assert_eq!(process.status().state, "error");
    let termination = process.terminate("测试回收");
    tokio::pin!(termination);
    assert!(
        futures::poll!(&mut termination).is_pending(),
        "首次 killpg 错误不能抢在 child.wait 的退出确认之前返回"
    );
    terminate_process_group(process.pid).unwrap();
    termination.await.unwrap();
    assert!(process.has_exited());
    assert_eq!(process.status().state, "closed");
}

#[tokio::test]
async fn confirmed_parent_exit_does_not_hide_process_group_permission_failure() {
    let (_fixture, process) = fake_process(Arc::new(|_, _| {}));
    process.abort_with("测试首次终止失败", |_| Err("injected EPERM".into()));
    terminate_process_group(process.pid).unwrap();
    let retries = AtomicUsize::new(0);
    let error = process
        .terminate_with("测试回收", |_| {
            retries.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Err("injected persistent EPERM".into()))
        })
        .await
        .unwrap_err();
    assert_eq!(retries.load(Ordering::SeqCst), 1);
    assert!(matches!(&*process.exited.borrow(), Some(Ok(()))));
    assert!(error.contains("persistent EPERM"));
    assert!(!process.has_exited(), "未确认进程组已停止，不允许替换服务");
    assert_eq!(process.status().state, "error");
    // 后续显式停止仍可重试；只有确认进程组已不存在才恢复 closed。
    process.terminate("测试最终回收").await.unwrap();
    assert!(process.has_exited());
}

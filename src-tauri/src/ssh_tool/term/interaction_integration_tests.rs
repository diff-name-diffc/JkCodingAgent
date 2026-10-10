//! 保活要求、条件等待、读屏历史与终端查询的 SSH 通道端到端回归。

use std::time::Duration;

use super::super::{TermError, TermReadOptions, TermWaitStatus, TmuxPreference};
use super::support::{fixture, fixture_with, open_params, Fixture, ServerBehavior};

/// 等待 reader 完成渲染而不调用 read，防止测试提前消费待验证的增量。
async fn wait_for_screen(fixture: &Fixture, term_id: &str, text: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if fixture
                .registry
                .screen_context_of(term_id)
                .is_some_and(|screen| screen.contains(text))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("远端输出应进入屏幕模型");
}

fn wait_options(pattern: &str) -> TermReadOptions {
    TermReadOptions {
        wait_ms: 5_000,
        wait_for: Some(pattern.into()),
        ..Default::default()
    }
}

#[tokio::test]
async fn required_tmux_refuses_missing_binary_before_running_command() {
    let fx = fixture(false).await;
    let mut params = open_params();
    params.tmux = TmuxPreference::Required;
    params.command = Some("perform-once".into());
    let error = fx
        .registry
        .open(&fx.manager, params)
        .await
        .expect_err("保活要求不能降为裸 PTY");
    let TermError::Open(message) = error else {
        panic!("缺少 tmux 应明确拒绝启动：{error:?}");
    };
    assert!(message.contains("安装") && message.contains("ssh_exec"));
    assert!(message.contains("tmux"));
    assert!(fx.registry.list(None).is_empty());
    assert_eq!(
        fx.events.lock().as_slice(),
        &["exec: command -v tmux"],
        "只能探测，不能启动 shell、新建会话或执行业务命令"
    );
}

#[tokio::test]
async fn required_tmux_opens_when_binary_is_available() {
    let fx = fixture(true).await;
    let mut params = open_params();
    params.tmux = TmuxPreference::Required;
    let opened = fx.registry.open(&fx.manager, params).await.expect("tmux");
    assert!(opened.tmux_session.is_some());
    assert!(opened.screen.contains("[tmux attached]"));
    assert!(!fx.events.lock().iter().any(|event| event == "shell"));
    fx.registry
        .close(&opened.term_id, true)
        .await
        .expect("kill");
}

#[tokio::test]
async fn send_does_not_consume_unread_completed_lines() {
    let fx = fixture(false).await;
    let opened = fx.registry.open(&fx.manager, open_params()).await.unwrap();
    fx.registry
        .send(&opened.term_id, "PENDING_BEFORE_SEND\r\n")
        .await
        .unwrap();
    wait_for_screen(&fx, &opened.term_id, "PENDING_BEFORE_SEND").await;
    fx.registry
        .send(&opened.term_id, "next input")
        .await
        .unwrap();
    let read = fx
        .registry
        .read(&opened.term_id, &TermReadOptions::default(), None)
        .await
        .unwrap();
    assert!(read
        .new_lines
        .iter()
        .any(|line| line.contains("PENDING_BEFORE_SEND")));
    fx.registry.close(&opened.term_id, false).await.unwrap();
}

#[tokio::test]
async fn resize_does_not_consume_unread_completed_lines() {
    let fx = fixture(false).await;
    let opened = fx.registry.open(&fx.manager, open_params()).await.unwrap();
    fx.registry
        .send(&opened.term_id, "PENDING_BEFORE_RESIZE\r\n")
        .await
        .unwrap();
    wait_for_screen(&fx, &opened.term_id, "PENDING_BEFORE_RESIZE").await;
    fx.registry.resize(&opened.term_id, 100, 30).await.unwrap();
    let read = fx
        .registry
        .read(&opened.term_id, &TermReadOptions::default(), None)
        .await
        .unwrap();
    assert!(read
        .new_lines
        .iter()
        .any(|line| line.contains("PENDING_BEFORE_RESIZE")));
    fx.registry.close(&opened.term_id, false).await.unwrap();
}

#[tokio::test]
async fn wait_for_ignores_unrelated_frames_and_matches_a_split_pattern() {
    let fx = fixture(false).await;
    let opened = fx.registry.open(&fx.manager, open_params()).await.unwrap();
    let options = wait_options("FRAME_TARGET");
    let read = fx.registry.read(&opened.term_id, &options, None);
    tokio::pin!(read);
    assert!(
        tokio::time::timeout(Duration::from_millis(30), read.as_mut())
            .await
            .is_err()
    );
    fx.registry
        .send(&opened.term_id, "unrelated output\r")
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), read.as_mut())
            .await
            .is_err()
    );
    fx.registry.send(&opened.term_id, "FRAME_").await.unwrap();
    wait_for_screen(&fx, &opened.term_id, "FRAME_").await;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), read.as_mut())
            .await
            .is_err()
    );
    fx.registry.send(&opened.term_id, "TARGET").await.unwrap();
    let matched = tokio::time::timeout(Duration::from_secs(3), read)
        .await
        .expect("完整模式应结束等待")
        .unwrap();
    assert_eq!(matched.wait_status, Some(TermWaitStatus::Matched));
    assert!(matched.screen.contains("FRAME_TARGET"));
    assert!(matched
        .new_lines
        .iter()
        .any(|line| line.contains("unrelated output")));
    fx.registry.close(&opened.term_id, false).await.unwrap();
}

#[tokio::test]
async fn wait_for_timeout_keeps_terminal_available_for_later_input() {
    let fx = fixture(false).await;
    let opened = fx.registry.open(&fx.manager, open_params()).await.unwrap();
    let options = TermReadOptions {
        wait_ms: 80,
        ..wait_options("not-yet-printed")
    };
    let timed_out = fx
        .registry
        .read(&opened.term_id, &options, None)
        .await
        .unwrap();
    assert_eq!(timed_out.wait_status, Some(TermWaitStatus::TimedOut));
    assert!(!timed_out.exited);
    fx.registry
        .send(&opened.term_id, "STILL_USABLE\r")
        .await
        .unwrap();
    let ready = fx
        .registry
        .read(&opened.term_id, &wait_options("STILL_USABLE"), None)
        .await
        .unwrap();
    assert_eq!(ready.wait_status, Some(TermWaitStatus::Matched));
    assert!(!ready.exited);
    fx.registry.close(&opened.term_id, false).await.unwrap();
}

#[tokio::test]
async fn wait_for_cancellation_stops_only_the_wait() {
    let fx = fixture(false).await;
    let opened = fx.registry.open(&fx.manager, open_params()).await.unwrap();
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let options = wait_options("not-yet-printed");
    let read = fx.registry.read(&opened.term_id, &options, Some(cancel_rx));
    tokio::pin!(read);
    assert!(
        tokio::time::timeout(Duration::from_millis(30), read.as_mut())
            .await
            .is_err()
    );
    cancel_tx.send(true).unwrap();
    let cancelled = tokio::time::timeout(Duration::from_secs(3), read)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cancelled.wait_status, Some(TermWaitStatus::Cancelled));
    assert!(!cancelled.exited);
    fx.registry
        .send(&opened.term_id, "AFTER_CANCEL\r")
        .await
        .unwrap();
    let ready = fx
        .registry
        .read(&opened.term_id, &wait_options("AFTER_CANCEL"), None)
        .await
        .unwrap();
    assert_eq!(ready.wait_status, Some(TermWaitStatus::Matched));
    fx.registry.close(&opened.term_id, false).await.unwrap();
}

#[tokio::test]
async fn wait_for_reports_remote_exit_with_exit_code() {
    let fx = fixture_with(ServerBehavior {
        exit_on_eof: Some(17),
        ..Default::default()
    })
    .await;
    let opened = fx.registry.open(&fx.manager, open_params()).await.unwrap();
    let options = wait_options("never-printed");
    let read = fx.registry.read(&opened.term_id, &options, None);
    tokio::pin!(read);
    assert!(
        tokio::time::timeout(Duration::from_millis(30), read.as_mut())
            .await
            .is_err()
    );
    fx.registry.send(&opened.term_id, "\x04").await.unwrap();
    let exited = tokio::time::timeout(Duration::from_secs(3), read)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exited.wait_status, Some(TermWaitStatus::Exited));
    assert!(exited.exited);
    assert_eq!(exited.exit_code, Some(17));
    fx.registry.close(&opened.term_id, false).await.unwrap();
}

#[tokio::test]
async fn open_preserves_early_command_output_for_read_and_wait_for() {
    let mut output = String::from("FIRST_COMMAND_MARKER\r\n");
    for line in 0..40 {
        output.push_str(&format!("later-output-{line:02}\r\n"));
    }
    let fx = fixture_with(ServerBehavior {
        command_output: Some(output.into_bytes()),
        ..Default::default()
    })
    .await;
    let mut params = open_params();
    params.rows = 10;
    params.command = Some("emit-many-lines".into());
    let opened = fx.registry.open(&fx.manager, params).await.unwrap();
    assert!(
        !opened.screen.contains("FIRST_COMMAND_MARKER"),
        "首行已滚出视口"
    );
    let read = fx
        .registry
        .read(&opened.term_id, &wait_options("FIRST_COMMAND_MARKER"), None)
        .await
        .unwrap();
    assert_eq!(read.wait_status, Some(TermWaitStatus::Matched));
    assert!(read
        .new_lines
        .iter()
        .any(|line| line == "FIRST_COMMAND_MARKER"));
    let drained = fx
        .registry
        .read(&opened.term_id, &TermReadOptions::default(), None)
        .await
        .unwrap();
    assert!(!drained
        .new_lines
        .iter()
        .any(|line| line == "FIRST_COMMAND_MARKER"));
    fx.registry.close(&opened.term_id, false).await.unwrap();
}

#[tokio::test]
async fn ansi_and_history_are_available_after_new_lines_are_consumed() {
    let fx = fixture(false).await;
    let mut params = open_params();
    params.rows = 10;
    let opened = fx.registry.open(&fx.manager, params).await.unwrap();
    let mut output = String::new();
    for line in 0..30 {
        output.push_str(&format!("\x1b[31mrow-{line:02}\x1b[0m\r\n"));
    }
    output.push_str("\x1b[31mRED_CURRENT\x1b[0m");
    fx.registry.send(&opened.term_id, &output).await.unwrap();
    let options = TermReadOptions {
        include_ansi: true,
        history: Some((0, 5)),
        ..wait_options("RED_CURRENT\n$")
    };
    let read = fx
        .registry
        .read(&opened.term_id, &options, None)
        .await
        .unwrap();
    assert_eq!(read.wait_status, Some(TermWaitStatus::Matched));
    assert!(!read.screen.contains('\x1b'));
    assert!(read.new_lines.iter().any(|line| line.contains("row-00")));
    let ansi = read.screen_ansi.as_deref().expect("ANSI 视口");
    assert!(ansi.contains("RED_CURRENT") && ansi.contains("38;5;1"));
    let history = read.history.expect("历史页");
    assert!(history.available_lines >= 20);
    assert_eq!(history.lines.len(), 5);
    assert_eq!(history.next_offset, Some(5));
    assert!(history
        .lines
        .iter()
        .all(|line| line.contains("row-") && line.contains("38;5;1")));

    let mut recovered = Vec::new();
    let mut offset = 0;
    loop {
        let options = TermReadOptions {
            history: Some((offset, 5)),
            ..Default::default()
        };
        let page = fx
            .registry
            .read(&opened.term_id, &options, None)
            .await
            .unwrap();
        assert!(page.screen_ansi.is_none());
        let history = page.history.unwrap();
        assert!(history.lines.iter().all(|line| !line.contains('\x1b')));
        recovered.extend(history.lines);
        match history.next_offset {
            Some(next) => {
                assert!(next > offset);
                offset = next;
            }
            None => break,
        }
    }
    assert!(
        recovered.iter().any(|line| line.contains("row-00")),
        "早期输出应仍可回溯"
    );
    assert_eq!(recovered.len(), history.available_lines);
    fx.registry.close(&opened.term_id, false).await.unwrap();
}

#[tokio::test]
async fn cursor_position_query_is_answered_over_the_ssh_channel() {
    let fx = fixture_with(ServerBehavior {
        query_cursor_position: true,
        ..Default::default()
    })
    .await;
    let opened = fx.registry.open(&fx.manager, open_params()).await.unwrap();
    let read = fx
        .registry
        .read(&opened.term_id, &wait_options("CPR_OK"), None)
        .await
        .unwrap();
    assert_eq!(read.wait_status, Some(TermWaitStatus::Matched));
    assert!(read.screen.contains("CPR_OK"));
    assert_eq!(
        fx.events
            .lock()
            .iter()
            .filter(|event| event.starts_with("cpr: "))
            .cloned()
            .collect::<Vec<_>>(),
        ["cpr: \x1b[4;7R"],
        "reader 必须真正将一基坐标应答写回 SSH 通道"
    );
    fx.registry.close(&opened.term_id, false).await.unwrap();
}

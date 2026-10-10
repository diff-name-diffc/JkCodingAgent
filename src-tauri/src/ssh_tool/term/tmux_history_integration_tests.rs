use super::super::{TermError, TermReadOptions};
use super::support::{fixture_with, open_params, ServerBehavior};

#[tokio::test]
async fn tmux_read_fetches_remote_history_without_typing_or_copy_mode() {
    let fx = fixture_with(ServerBehavior {
        tmux_available: true,
        eof_before_status: true,
        command_output: Some(b"4\n\x1b[31mold\x1b[39m\nnew\n".to_vec()),
        ..Default::default()
    })
    .await;
    let opened = fx.registry.open(&fx.manager, open_params()).await.unwrap();
    let read = fx
        .registry
        .read(
            &opened.term_id,
            &TermReadOptions {
                include_ansi: true,
                history: Some((0, 2)),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    let history = read.history.unwrap();
    assert_eq!(history.source, "tmux");
    assert_eq!(history.available_lines, 4);
    assert_eq!(history.next_offset, Some(2));
    assert!(history.lines[0].contains("old"));
    assert!(history.lines[0].contains("38;5;1"));
    assert!(read.screen.contains("[tmux attached]"));
    let name = opened.tmux_session.unwrap();
    assert!(fx.events.lock().iter().any(|event| event == &format!(
        "exec: tmux display-message -p -t ={name}: '#{{history_size}}' \\; capture-pane -p -e -t ={name}: -S -2 -E -1"
    )));
    fx.registry.close(&opened.term_id, false).await.unwrap();
}

#[tokio::test]
async fn tmux_history_transport_failures_are_not_empty_pages() {
    for (behavior, message) in [
        (
            ServerBehavior {
                history_exit: 1,
                ..Default::default()
            },
            "can't find pane",
        ),
        (
            ServerBehavior {
                history_omit_status: true,
                ..Default::default()
            },
            "未取得退出码",
        ),
        (
            ServerBehavior {
                command_output: Some(vec![b'x'; 1_048_577]),
                ..Default::default()
            },
            "传输上限",
        ),
    ] {
        let fx = fixture_with(ServerBehavior {
            tmux_available: true,
            ..behavior
        })
        .await;
        let opened = fx.registry.open(&fx.manager, open_params()).await.unwrap();
        let error = fx
            .registry
            .read(
                &opened.term_id,
                &TermReadOptions {
                    history: Some((0, 2)),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(error, TermError::Read(detail) if detail.contains(message)));
        fx.registry.close(&opened.term_id, false).await.unwrap();
    }
}

#[tokio::test]
async fn failed_history_read_keeps_unread_terminal_output() {
    let fx = fixture_with(ServerBehavior {
        tmux_available: true,
        command_output: Some(b"invalid metadata\n".to_vec()),
        ..Default::default()
    })
    .await;
    let opened = fx.registry.open(&fx.manager, open_params()).await.unwrap();
    let error = fx
        .registry
        .read(
            &opened.term_id,
            &TermReadOptions {
                history: Some((0, 10)),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, TermError::Read(message) if message.contains("元数据无效")));
    let read = fx
        .registry
        .read(&opened.term_id, &TermReadOptions::default(), None)
        .await
        .unwrap();
    assert!(read
        .new_lines
        .iter()
        .any(|line| line.contains("[tmux attached]")));
    fx.registry.close(&opened.term_id, false).await.unwrap();
}

#[tokio::test]
async fn cancelled_wait_does_not_start_remote_history_read() {
    let fx = fixture_with(ServerBehavior {
        tmux_available: true,
        ..Default::default()
    })
    .await;
    let opened = fx.registry.open(&fx.manager, open_params()).await.unwrap();
    let (_, cancel) = tokio::sync::watch::channel(true);
    let read = fx
        .registry
        .read(
            &opened.term_id,
            &TermReadOptions {
                wait_ms: 25_000,
                wait_for: Some("unseen prompt".into()),
                history: Some((0, 2)),
                ..Default::default()
            },
            Some(cancel),
        )
        .await
        .unwrap();
    assert_eq!(
        read.wait_status,
        Some(super::super::TermWaitStatus::Cancelled)
    );
    assert!(read.history.is_none());
    assert!(read.note.unwrap().contains("未查询历史"));
    assert!(!fx
        .events
        .lock()
        .iter()
        .any(|event| event.contains("capture-pane")));
    let (_, cancel) = tokio::sync::watch::channel(true);
    let error = fx
        .registry
        .read(
            &opened.term_id,
            &TermReadOptions {
                history: Some((0, 2)),
                ..Default::default()
            },
            Some(cancel),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, TermError::Read(message) if message.contains("未发起远端查询")));
    assert!(!fx
        .events
        .lock()
        .iter()
        .any(|event| event.contains("capture-pane")));
    fx.registry.close(&opened.term_id, false).await.unwrap();
}

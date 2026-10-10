//! 启动分阶段预算与确认丢失后的副作用边界。

use std::time::Duration;

use super::super::{TermError, TmuxPreference};
use super::support::{fixture_with, open_params, Fixture, ServerBehavior};

#[tokio::test]
async fn same_name_restore_survives_accumulated_round_trip_latency() {
    let fx = fixture_with(ServerBehavior {
        tmux_available: true,
        exec_reply_delay: Duration::from_secs(2),
        ..Default::default()
    })
    .await;
    let name = "jkagent-latent-restore";
    // detach 后远端会话仍存活；恢复需 probe、has-session、attach 三次往返。
    fx.tmux_sessions.lock().insert(name.into());
    let mut params = open_params();
    params.tmux_session = Some(name.into());
    params.command = Some("must-not-run-again".into());
    let opened = fx
        .registry
        .open(&fx.manager, params)
        .await
        .expect("每次往返均低于 5s 时，累计 6s 不应耗尽 attach 预算");
    assert_eq!(opened.tmux_session.as_deref(), Some(name));
    assert!(opened.note.as_deref().unwrap().contains("已恢复"));
    assert_eq!(
        fx.events.lock().as_slice(),
        [
            "exec: command -v tmux",
            &format!("exec: tmux has-session -t ={name}"),
            &format!("exec: tmux attach-session -t ={name}"),
        ],
        "同名恢复不得重发 command 或新建 tmux 会话"
    );
    fx.registry
        .close(&opened.term_id, false)
        .await
        .expect("detach");
}

#[tokio::test]
async fn timed_out_tmux_attach_closes_channel_without_replaying_command() {
    assert_timeout_preserves_unknown_command_state(true).await;
}

#[tokio::test]
async fn timed_out_bare_command_closes_channel_without_replaying_command() {
    assert_timeout_preserves_unknown_command_state(false).await;
}

async fn assert_timeout_preserves_unknown_command_state(tmux_available: bool) {
    let fx = fixture_with(ServerBehavior {
        tmux_available,
        omit_terminal_reply: true,
        ..Default::default()
    })
    .await;
    let mut params = open_params();
    params.command = Some("perform-once".into());
    params.tmux = if tmux_available {
        TmuxPreference::Required
    } else {
        TmuxPreference::Off
    };
    let error = tokio::time::timeout(
        Duration::from_secs(10),
        fx.registry.open(&fx.manager, params),
    )
    .await
    .expect("缺失确认仍须有界失败")
    .expect_err("服务端没有确认启动");
    assert!(matches!(error, TermError::ExternalStateUnknown(message)
        if message.contains("超时") && message.contains("禁止自动重跑")));
    assert!(fx.registry.list(None).is_empty());
    {
        let events = fx.events.lock();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.contains("perform-once"))
                .count(),
            1,
            "超时后不得重发可能已执行的命令"
        );
    }
    await_failed_channel_close(&fx).await;
}

async fn await_failed_channel_close(fx: &Fixture) {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let closed = {
                let events = fx.events.lock();
                let channel = events
                    .iter()
                    .find_map(|event| event.strip_prefix("withheld-reply: "))
                    .expect("回环服务器已收到启动请求");
                events
                    .iter()
                    .any(|event| event == &format!("close: {channel}"))
            };
            if closed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("超时后的清理必须另有预算向服务端发送 Close");
}

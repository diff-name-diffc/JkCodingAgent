//! ssh_term_* 集成测试：russh 回环 server 起真实 PTY 通道，覆盖
//! open（shell / exec / tmux 分路）→ send → read → close 全链路。
//!
//! 服务器行为模拟（非真实 shell）：shell 模式回显输入并在 `\r` 后补提示符；
//! exec 模式按命令名脚本化应答（`command -v tmux` 探测 / tmux attach / 普通命令）。

#[path = "test_support.rs"]
mod support;

#[path = "interaction_integration_tests.rs"]
mod interaction_integration;

#[path = "paste_integration_tests.rs"]
mod paste_integration;

#[path = "tmux_history_integration_tests.rs"]
mod tmux_history_integration;

#[path = "startup_integration_tests.rs"]
mod startup_integration;

use support::{fixture, fixture_with, open_params, ServerBehavior};

// ---------------------------------------------------------------------------
// 用例
// ---------------------------------------------------------------------------

#[tokio::test]
async fn open_shell_reads_prompt_without_tmux() {
    let fx = fixture(false).await;
    let payload = fx
        .registry
        .open(&fx.manager, open_params())
        .await
        .expect("open");
    assert!(
        payload.tmux_session.is_none(),
        "确认远端未安装 tmux 后可打开裸 shell"
    );
    assert!(
        payload.screen.contains('$'),
        "首屏应含提示符：{:?}",
        payload.screen
    );
    assert!(!payload.exited);
    assert!(payload.note.as_deref().unwrap().contains("不支持保活"));
    // 收尾
    let _ = fx.registry.close(&payload.term_id, false).await;
}

#[tokio::test]
async fn send_read_roundtrip_finalizes_echo_lines() {
    let fx = fixture(false).await;
    let opened = fx
        .registry
        .open(&fx.manager, open_params())
        .await
        .expect("open");
    // open 的首屏等待已消费提示符帧，先排空再发送。
    let _ = fx
        .registry
        .read(
            &opened.term_id,
            &super::TermReadOptions {
                wait_ms: 0,
                ..Default::default()
            },
            None,
        )
        .await;
    fx.registry
        .send(&opened.term_id, "echo hi\r")
        .await
        .expect("send");
    let read = fx
        .registry
        .read(
            &opened.term_id,
            &super::TermReadOptions {
                wait_ms: 3_000,
                ..Default::default()
            },
            None,
        )
        .await
        .expect("read");
    // 回显行进入增量轨（光标行定稿）。
    assert!(
        read.new_lines.iter().any(|line| line.contains("echo hi")),
        "增量轨应含命令回显：{:?}",
        read.new_lines
    );
    assert!(read.screen.contains('$'), "屏幕应回到提示符");
    let _ = fx.registry.close(&opened.term_id, false).await;
}

#[tokio::test]
async fn open_with_command_runs_exec_path_and_exits() {
    let fx = fixture(false).await;
    let mut params = open_params();
    params.command = Some("htop".to_string());
    let payload = fx.registry.open(&fx.manager, params).await.expect("open");
    assert!(
        payload.screen.contains("run: htop"),
        "exec 输出应上屏：{:?}",
        payload.screen
    );
    // 回环服务器 exec 完成即上报 exit 0 + eof：等 reader 收口。
    let read = fx
        .registry
        .read(
            &payload.term_id,
            &super::TermReadOptions {
                wait_ms: 2_000,
                ..Default::default()
            },
            None,
        )
        .await
        .expect("read");
    assert!(read.exited, "命令结束应标记 exited");
    assert_eq!(read.exit_code, Some(0));
    // 再发送应被拒绝。
    let err = fx
        .registry
        .send(&payload.term_id, "x")
        .await
        .expect_err("exited send");
    assert!(matches!(err, super::registry::TermError::Exited(_)));
}

#[tokio::test]
async fn tmux_auto_attach_or_create() {
    let fx = fixture(true).await;
    let payload = fx
        .registry
        .open(&fx.manager, open_params())
        .await
        .expect("open");
    let name = payload.tmux_session.as_deref().expect("应进入 tmux 会话");
    assert!(name.starts_with("jkagent-"), "tmux 会话名前缀：{name}");
    assert!(
        payload.screen.contains("[tmux attached]"),
        "tmux attach 输出应上屏：{:?}",
        payload.screen
    );
    // 同名 open 恢复现场（attach-or-create 语义）。
    let mut params = open_params();
    params.tmux_session = Some(name.to_string());
    let reopened = fx.registry.open(&fx.manager, params).await.expect("reopen");
    assert_eq!(reopened.tmux_session.as_deref(), Some(name));
    let _ = fx.registry.close(&payload.term_id, false).await;
    let _ = fx.registry.close(&reopened.term_id, false).await;
}

#[tokio::test]
async fn close_is_idempotent_and_reports_exit() {
    let fx = fixture(false).await;
    let opened = fx
        .registry
        .open(&fx.manager, open_params())
        .await
        .expect("open");
    let closed = fx
        .registry
        .close(&opened.term_id, false)
        .await
        .expect("close");
    assert_eq!(closed.term_id, opened.term_id);
    // 幂等：再次 close 报 NotFound。
    let err = fx
        .registry
        .close(&opened.term_id, false)
        .await
        .expect_err("double close");
    assert!(matches!(err, super::registry::TermError::NotFound(_)));
    assert!(fx.registry.list(None).is_empty());
}

#[tokio::test]
async fn custom_tmux_name_validated_and_quota_reports() {
    let fx = fixture(true).await;
    // 非法名（缺前缀）在 open 即被拒。
    let mut params = open_params();
    params.tmux_session = Some("evil; rm -rf".to_string());
    let err = fx
        .registry
        .open(&fx.manager, params)
        .await
        .expect_err("非法 tmux 会话名");
    assert!(matches!(err, super::registry::TermError::Open(_)));

    // 每服务器配额 ≤4：开满后第 5 个被拒且报错附清单。
    let mut ids = Vec::new();
    for _ in 0..4 {
        let payload = fx
            .registry
            .open(&fx.manager, open_params())
            .await
            .expect("open quota");
        ids.push(payload.term_id);
    }
    let err = fx
        .registry
        .open(&fx.manager, open_params())
        .await
        .expect_err("超配额");
    match err {
        super::registry::TermError::QuotaExceeded(message) => {
            assert!(
                message.contains("loop-server"),
                "报错应附会话清单：{message}"
            );
        }
        other => panic!("期望配额错误，实际 {other:?}"),
    }
    for id in ids {
        let _ = fx.registry.close(&id, false).await;
    }
}

#[tokio::test]
async fn resize_propagates_window_change_to_remote() {
    let fx = fixture(false).await;
    let opened = fx
        .registry
        .open(&fx.manager, open_params())
        .await
        .expect("open");
    fx.registry
        .resize(&opened.term_id, 120, 40)
        .await
        .expect("resize");
    // 等 window_change 事件传播到回环 server。
    for _ in 0..50 {
        if fx
            .events
            .lock()
            .iter()
            .any(|e| e == "window-change: 120x40")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        fx.events
            .lock()
            .iter()
            .any(|e| e == "window-change: 120x40"),
        "远端应收到 window_change：{:?}",
        fx.events.lock()
    );
    // 调整后读屏正常（快照轨照常组装）。
    let read = fx
        .registry
        .read(
            &opened.term_id,
            &super::TermReadOptions {
                wait_ms: 0,
                ..Default::default()
            },
            None,
        )
        .await
        .expect("read");
    assert!(!read.screen.is_empty());
    let _ = fx.registry.close(&opened.term_id, false).await;
}

#[tokio::test]
async fn orphan_tmux_session_reclaimed_via_audit_backfill() {
    let fx = fixture(true).await;
    let opened = fx
        .registry
        .open(&fx.manager, open_params())
        .await
        .expect("open");
    let name = opened.tmux_session.clone().expect("tmux 会话名");
    // 模拟失联：detach 关闭（注册表移除、远端 tmux 现场保留）——正是孤儿场景
    // 的真实路径（空闲回收 / 断连后同样只剩远端会话）。
    fx.registry
        .close(&opened.term_id, false)
        .await
        .expect("detach");
    assert!(fx.registry.list(None).is_empty(), "注册表应已无该会话");
    // 测试直连 registry 不经工具层，手动补 open 审计（孤儿反查的数据源）。
    fx.manager
        .append_term_activity_audit(
            std::path::PathBuf::from("/tmp/ws"),
            "ws".to_string(),
            "orphan-test".to_string(),
            "loop-server".to_string(),
            "it-session".to_string(),
            format!("ssh_term_open tmux new -A -s {name} (80x24)"),
            None,
        )
        .await
        .expect("补审计");
    // 级联清理：注册表已无该会话，孤儿回收经审计反查 kill 远端。
    fx.registry
        .close_session_terms(&fx.manager, "it-session")
        .await;
    for _ in 0..50 {
        if fx
            .events
            .lock()
            .iter()
            .any(|e| e == &format!("exec: tmux kill-session -t ={name}"))
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        fx.events
            .lock()
            .iter()
            .any(|e| e == &format!("exec: tmux kill-session -t ={name}")),
        "孤儿 tmux 会话应被反查 kill：{:?}",
        fx.events.lock()
    );
}

#[tokio::test]
async fn eof_before_exit_status_does_not_disable_tmux() {
    let fx = fixture_with(ServerBehavior {
        tmux_available: true,
        eof_before_status: true,
        ..Default::default()
    })
    .await;
    let opened = fx
        .registry
        .open(&fx.manager, open_params())
        .await
        .expect("open tmux");
    assert!(opened.tmux_session.is_some(), "EOF 不能被当作探测失败");
    assert!(!fx.events.lock().iter().any(|event| event == "shell"));
    fx.registry
        .close(&opened.term_id, true)
        .await
        .expect("kill");
    assert!(fx.tmux_sessions.lock().is_empty());
}

#[tokio::test]
async fn missing_exit_status_or_probe_error_never_falls_back_to_shell() {
    for behavior in [
        ServerBehavior {
            omit_probe_status: true,
            ..Default::default()
        },
        ServerBehavior {
            probe_exit: Some(2),
            ..Default::default()
        },
    ] {
        let fx = fixture_with(behavior).await;
        let error = fx
            .registry
            .open(&fx.manager, open_params())
            .await
            .expect_err("探测必须有可靠结果");
        assert!(matches!(error, super::TermError::Open(_)));
        assert!(!fx.events.lock().iter().any(|event| event == "shell"));
        assert!(fx.registry.list(None).is_empty());
    }
}

#[tokio::test]
async fn explicit_tmux_name_cannot_silently_open_bare_shell() {
    let fx = fixture(false).await;
    for preference in [super::TmuxPreference::Auto, super::TmuxPreference::Off] {
        let mut params = open_params();
        params.tmux_session = Some("jkagent-recover".into());
        params.tmux = preference;
        fx.registry
            .open(&fx.manager, params)
            .await
            .expect_err("指定名称不得忽略");
    }
    assert!(!fx.events.lock().iter().any(|event| event == "shell"));
    assert!(fx.registry.list(None).is_empty());
}

#[tokio::test]
async fn custom_session_detaches_restores_then_kills_without_false_recovery_note() {
    let fx = fixture(true).await;
    let mut params = open_params();
    params.tmux_session = Some("jkagent-persistent".into());
    let first = fx.registry.open(&fx.manager, params).await.expect("create");
    assert!(first.note.as_deref().unwrap().contains("已新建"));
    fx.registry
        .close(&first.term_id, false)
        .await
        .expect("detach");
    assert!(fx.tmux_sessions.lock().contains("jkagent-persistent"));
    let mut params = open_params();
    params.tmux_session = Some("jkagent-persistent".into());
    let restored = fx
        .registry
        .open(&fx.manager, params)
        .await
        .expect("restore");
    assert!(restored.note.as_deref().unwrap().contains("已恢复"));
    assert_eq!(
        fx.events
            .lock()
            .iter()
            .filter(|event| event.starts_with("exec: tmux new-session"))
            .count(),
        1
    );
    let closed = fx
        .registry
        .close(&restored.term_id, true)
        .await
        .expect("kill");
    assert!(fx.tmux_sessions.lock().is_empty());
    assert!(closed.note.as_deref().unwrap().contains("已回收"));
    assert!(!closed.note.as_deref().unwrap().contains("可恢复"));
}

#[tokio::test]
async fn reviewed_command_runs_in_new_tmux_and_is_not_replayed_on_restore() {
    let fx = fixture(true).await;
    for _ in 0..2 {
        let mut params = open_params();
        params.tmux_session = Some("jkagent-command".into());
        params.command = Some("printf '%s' \"$HOME\"".into());
        let opened = fx
            .registry
            .open(&fx.manager, params)
            .await
            .expect("tmux command");
        assert_eq!(opened.tmux_session.as_deref(), Some("jkagent-command"));
        fx.registry
            .close(&opened.term_id, false)
            .await
            .expect("detach");
    }
    let events = fx.events.lock();
    let creates: Vec<_> = events
        .iter()
        .filter(|event| event.starts_with("exec: tmux new-session"))
        .collect();
    assert_eq!(creates.len(), 1);
    assert!(creates[0].contains("'printf '\\''%s'\\'' \"$HOME\"'"));
    assert!(!events.iter().any(|event| event.starts_with("exec: printf")));
}

#[tokio::test]
async fn rejected_pty_shell_or_tmux_creation_is_an_open_error() {
    for behavior in [
        ServerBehavior {
            reject_pty: true,
            ..Default::default()
        },
        ServerBehavior {
            reject_shell: true,
            ..Default::default()
        },
        ServerBehavior {
            tmux_available: true,
            create_exit: 1,
            ..Default::default()
        },
    ] {
        let fx = fixture_with(behavior).await;
        fx.registry
            .open(&fx.manager, open_params())
            .await
            .expect_err("启动失败必须报错");
        assert!(fx.registry.list(None).is_empty());
        assert!(!fx
            .events
            .lock()
            .iter()
            .any(|event| event.starts_with("exec: tmux attach-session")));
    }
}

#[tokio::test]
async fn failed_tmux_kill_keeps_handle_and_reports_remote_failure() {
    let fx = fixture_with(ServerBehavior {
        tmux_available: true,
        kill_exit: 1,
        ..Default::default()
    })
    .await;
    let opened = fx
        .registry
        .open(&fx.manager, open_params())
        .await
        .expect("open");
    let error = fx
        .registry
        .close(&opened.term_id, true)
        .await
        .expect_err("kill failed");
    assert!(matches!(error, super::TermError::Write(message) if message.contains("kill denied")));
    assert_eq!(fx.registry.list(None).len(), 1);
    assert_eq!(fx.tmux_sessions.lock().len(), 1);
    fx.registry
        .close(&opened.term_id, false)
        .await
        .expect("detach remains available");
}

#[tokio::test]
async fn lost_create_receipt_or_early_command_exit_must_not_replay_side_effects() {
    for behavior in [
        ServerBehavior {
            tmux_available: true,
            create_omit_status: true,
            ..Default::default()
        },
        ServerBehavior {
            tmux_available: true,
            attach_exit: Some(1),
            ..Default::default()
        },
    ] {
        let fx = fixture_with(behavior).await;
        let mut params = open_params();
        params.tmux_session = Some("jkagent-uncertain".into());
        params.command = Some("perform-once".into());
        let error = fx
            .registry
            .open(&fx.manager, params)
            .await
            .expect_err("远端命令可能已执行，不能返回可重试的普通错误");
        assert!(matches!(
            error,
            super::TermError::ExternalStateUnknown(message)
                if message.contains("jkagent-uncertain") && message.contains("禁止")
        ));
        assert!(fx.registry.list(None).is_empty());
        let events = fx.events.lock();
        assert!(!events.iter().any(|event| event == "shell"));
        assert_eq!(
            events
                .iter()
                .filter(|event| event.starts_with("exec: tmux new-session"))
                .count(),
            1,
            "发生确认丢失后不得自动重发 command",
        );
        assert!(!events.iter().any(|event| event == "exec: perform-once"));
    }
}

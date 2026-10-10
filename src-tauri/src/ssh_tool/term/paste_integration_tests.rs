//! 使用回环 SSH 的真实 channel 验证粘贴能力门禁，不用 fixture 冒充 readline。

use super::super::{TermError, TermReadOptions};
use super::support::{fixture, open_params};

#[tokio::test]
async fn paste_requires_remote_enable_and_rechecks_before_each_write() {
    let fx = fixture(false).await;
    let opened = fx.registry.open(&fx.manager, open_params()).await.unwrap();
    let term_id = &opened.term_id;
    let paste = "\x1b[200~PASTED_FIRST\nPASTED_SECOND\x1b[201~\r";
    let error = fx
        .registry
        .send_checked(term_id, paste, true)
        .await
        .unwrap_err();
    assert!(matches!(error, TermError::Write(message) if message.contains("未启用")));

    // 回环服务器原样回显，显式输出终端模式；MODE_READY 出现时该帧已被解析。
    fx.registry
        .send(term_id, "\x1b[?2004hMODE_READY\r\n")
        .await
        .unwrap();
    let read = fx
        .registry
        .read(
            term_id,
            &TermReadOptions {
                wait_ms: 3_000,
                wait_for: Some("MODE_READY".into()),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    assert!(
        !read.screen.contains("PASTED_FIRST"),
        "拒绝时不得发送任何载荷"
    );
    fx.registry.validate_paste_state(term_id).unwrap();
    fx.registry
        .send_checked(term_id, paste, true)
        .await
        .unwrap();
    let read = fx
        .registry
        .read(
            term_id,
            &TermReadOptions {
                wait_ms: 3_000,
                wait_for: Some("PASTED_SECOND".into()),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    assert!(read.screen.contains("PASTED_FIRST"));
    assert!(read.screen.contains("PASTED_SECOND"));

    // 模拟送审期间远端结束 readline；即使前置检查通过，写入也必须重新拒绝。
    fx.registry
        .send(term_id, "\x1b[?2004lMODE_DISABLED\r\n")
        .await
        .unwrap();
    fx.registry
        .read(
            term_id,
            &TermReadOptions {
                wait_ms: 3_000,
                wait_for: Some("MODE_DISABLED".into()),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    let error = fx
        .registry
        .send_checked(term_id, "\x1b[200~NEVER_SENT\x1b[201~", true)
        .await
        .unwrap_err();
    assert!(matches!(error, TermError::Write(message) if message.contains("未启用")));
    fx.registry.send(term_id, "NORMAL_KEYS\r\n").await.unwrap();
    let read = fx
        .registry
        .read(
            term_id,
            &TermReadOptions {
                wait_ms: 3_000,
                wait_for: Some("NORMAL_KEYS".into()),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    assert!(!read.screen.contains("NEVER_SENT"));
    assert!(
        read.screen.contains("NORMAL_KEYS"),
        "普通按键不需要粘贴能力"
    );
    fx.registry.close(term_id, false).await.unwrap();
}

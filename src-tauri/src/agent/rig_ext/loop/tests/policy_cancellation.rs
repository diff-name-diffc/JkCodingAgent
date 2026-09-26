use super::*;
use crate::agent::rig_ext::r#loop::invocation::ToolInvocationContext;
use rig::message::{ToolCall, ToolFunction};
use std::time::Duration;
use tokio::sync::{oneshot, Notify};

#[tokio::test(start_paused = true)]
async fn unified_timeout_signals_callback_and_waits_for_actual_settlement() {
    let fixture = Fixture::new();
    let events = Channel::new(|_| Ok(()));
    let policy = AppToolExecutionPolicy::new(
        &fixture.db,
        &events,
        AppToolPolicyConfig {
            workspace_id: fixture.workspace_id.clone(),
            workspace: fixture.temp_dir.clone(),
            review: crate::agent::rig_ext::review::RigReviewContext::unconfigured(),
            cancel_rx: None,
            trace: Default::default(),
        },
    );
    let (noticed, cancellation) = oneshot::channel();
    let noticed = Arc::new(Mutex::new(Some(noticed)));
    let release = Arc::new(Notify::new());
    let tool_release = release.clone();
    let tool = PortableDynamicTool::new(
        "read_file",
        "test",
        serde_json::json!({"type":"object"}),
        move |_| {
            let (noticed, release) = (noticed.clone(), tool_release.clone());
            Box::pin(async move {
                let mut rx = ToolInvocationContext::current().unwrap().cancel_rx;
                while !*rx.borrow() {
                    rx.changed().await.unwrap();
                }
                noticed.lock().take().unwrap().send(()).unwrap();
                release.notified().await;
                Ok(ToolOutput::text("settled"))
            })
        },
    );
    let (_cancel, cancel_rx) = watch::channel(false);
    let context = ToolInvocationContext {
        workspace_id: fixture.workspace_id.clone(),
        agent_run_id: "run".into(),
        task_id: "task".into(),
        tool_call_id: "call".into(),
        root_request_message_id: "request".into(),
        cancel_rx,
    };
    let call = ToolCall::from_wire(
        "call",
        ToolFunction {
            name: "read_file".into(),
            arguments: serde_json::json!({}),
        },
    );
    let task = tokio::spawn(context.scope(async move { policy.execute(&tool, &call).await }));
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    cancellation.await.unwrap();
    assert!(
        !task.is_finished(),
        "超时不能丢弃仍持有本地写入路径的 future"
    );
    release.notify_one();
    assert_eq!(task.await.unwrap().unwrap_err().retryable(), Some(false));
}

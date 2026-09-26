use super::*;
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler, ServiceExt};
use std::sync::Arc;
use tokio::sync::{watch, Notify};

struct BlockingServer {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

impl ServerHandler for BlockingServer {
    async fn call_tool(
        &self,
        _: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.started.notify_one();
        self.release.notified().await;
        Ok(CallToolResult::success(vec![]))
    }
}

struct ImmediateServer;

impl ServerHandler for ImmediateServer {
    async fn call_tool(
        &self,
        _: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(CallToolResult::success(vec![rmcp::model::Content::text(
            "ok",
        )]))
    }
}

#[tokio::test]
async fn cancelling_an_inflight_request_settles_transport_and_reports_unknown_state() {
    let started = Arc::new(Notify::new());
    let (server_io, client_io) = tokio::io::duplex(4096);
    let server_started = started.clone();
    let release = Arc::new(Notify::new());
    let server_release = release.clone();
    let server = tokio::spawn(async move {
        BlockingServer {
            started: server_started,
            release: server_release,
        }
        .serve(server_io)
        .await
        .unwrap()
        .waiting()
        .await
    });
    let client = ().serve(client_io).await.unwrap();
    let (cancel, cancel_rx) = watch::channel(false);
    // 取消源显式注入：底层不再自行读取 agent 循环的 task-local。
    let task = tokio::spawn(execute(
        client,
        CallToolRequestParams::new("slow"),
        Duration::from_secs(60),
        Some(cancel_rx),
    ));
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    cancel.send_replace(true);
    let result = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    let error = result.unwrap_err();
    assert!(error.is_external_state_unknown());
    assert!(error.to_string().contains("external_state_unknown"));
    // 外部服务可以忽略取消；宿主只保证自己的 transport 已收敛。
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn successful_call_returns_result_even_when_transport_settle_fails() {
    // 纯逻辑：收尾失败绝不覆盖 Ok，也不改变失败结果的类别。
    let settled = merge_settle_outcome(
        Ok(CallToolResult::success(vec![])),
        Some("MCP transport 收尾失败：boom".into()),
    );
    assert!(settled.is_ok());

    let error = merge_settle_outcome(
        Err(McpCallError::external_state_unknown(
            "external_state_unknown：MCP 调用失败",
        )),
        Some("MCP transport 收尾失败：boom".into()),
    )
    .unwrap_err();
    assert!(error.is_external_state_unknown());
    assert!(error.to_string().contains("MCP 调用失败"));
    assert!(error.to_string().contains("transport 收尾失败"));

    // 全链路：正常响应的 server 走成功路径，transport 收尾不影响结果。
    let (server_io, client_io) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move {
        ImmediateServer
            .serve(server_io)
            .await
            .unwrap()
            .waiting()
            .await
    });
    let client = ().serve(client_io).await.unwrap();
    // 非 agent 路径（注册表检查等）：不注入取消源，调用照常执行。
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        execute(
            client,
            CallToolRequestParams::new("fast"),
            Duration::from_secs(5),
            None,
        ),
    )
    .await
    .unwrap();
    assert!(result.is_ok());
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn pre_cancelled_call_is_not_sent_and_not_marked_unknown_state() {
    let (server_io, client_io) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move {
        BlockingServer {
            started: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
        }
        .serve(server_io)
        .await
        .unwrap()
        .waiting()
        .await
    });
    let client = ().serve(client_io).await.unwrap();
    let (_cancel, cancel_rx) = watch::channel(true);
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        execute(
            client,
            CallToolRequestParams::new("never-sent"),
            Duration::from_secs(60),
            Some(cancel_rx),
        ),
    )
    .await
    .unwrap();
    let error = result.unwrap_err();
    assert!(!error.is_external_state_unknown());
    assert!(error.to_string().contains("未发送工具请求"));
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn exhausted_remaining_budget_does_not_send_the_request() {
    // 共享预算被初始化握手吃光 → 剩余 0：不得发出注定超时的请求，按
    // 「未发送」（可重试）处理，而不是发出去再标 external_state_unknown。
    let started = Arc::new(Notify::new());
    let (server_io, client_io) = tokio::io::duplex(4096);
    let server = tokio::spawn({
        let started = started.clone();
        async move {
            BlockingServer {
                started,
                release: Arc::new(Notify::new()),
            }
            .serve(server_io)
            .await
            .unwrap()
            .waiting()
            .await
        }
    });
    let client = ().serve(client_io).await.unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        execute(
            client,
            CallToolRequestParams::new("never-sent"),
            Duration::ZERO,
            None,
        ),
    )
    .await
    .unwrap();
    let error = result.unwrap_err();
    assert!(!error.is_external_state_unknown());
    assert!(error.to_string().contains("预算"));
    // 请求确实没有发出：server 从未收到调用。
    assert!(
        tokio::time::timeout(Duration::from_millis(100), started.notified())
            .await
            .is_err()
    );
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn hung_server_does_not_block_worker_beyond_settle_timeout() {
    // server 收到请求后永不响应：调用超时 → 取消通知（限时）→ transport
    // 收尾（限时）。注入短超时验证整个路径有界、不挂死 worker。
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (server_io, client_io) = tokio::io::duplex(4096);
    let server = tokio::spawn({
        let started = started.clone();
        let release = release.clone();
        async move {
            BlockingServer { started, release }
                .serve(server_io)
                .await
                .unwrap()
                .waiting()
                .await
        }
    });
    let client = ().serve(client_io).await.unwrap();
    let task = tokio::spawn(execute_with_settle_timeout(
        client,
        CallToolRequestParams::new("slow"),
        Duration::from_millis(100),
        Duration::from_millis(100),
        None,
    ));
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    // 不发取消信号、server 永不响应：仅靠注入的短超时收尾。
    // 若取消通知或 transport 收敛没有超时上限，这里会挂死直到外层超时失败。
    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    let error = result.unwrap_err();
    assert!(error.is_external_state_unknown());
    assert!(error.to_string().contains("取消通知结果"));
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

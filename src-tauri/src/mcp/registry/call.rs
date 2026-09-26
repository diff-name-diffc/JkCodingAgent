//! 请求取消通知与 transport 收敛必须一起完成，不能直接丢弃执行 future。
//!
//! 取消源由调用方注入（tool 边界取一次 agent 循环的 task-local 后传入）：
//! 本模块不 import agent 循环类型，非 agent 路径（注册表检查等）直接传 None。
use std::fmt;
use std::time::Duration;

use rmcp::model::{CallToolRequestParams, CallToolResult, ClientRequest, Request, ServerResult};
use rmcp::service::{PeerRequestOptions, RunningService};
use rmcp::RoleClient;
use tokio::sync::watch;

#[cfg(test)]
mod tests;

/// 取消通知与 transport 收敛的等待上限：挂死的 server 不得永久挂住 worker
/// （旧实现靠外层 startup_timeout 整体兜底，现在逐点限时）。
const CANCEL_SETTLE_TIMEOUT: Duration = Duration::from_secs(5);

/// MCP 调用错误：message 为模型/人类可读文案；external_state_unknown 标记
/// 「请求已发送、外部状态未知」（调用方据此禁止自动重试），false 表示请求
/// 未发送（按普通失败处理）。
pub(crate) struct McpCallError {
    message: String,
    external_state_unknown: bool,
}

impl McpCallError {
    pub(crate) fn not_sent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            external_state_unknown: false,
        }
    }

    pub(crate) fn external_state_unknown(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            external_state_unknown: true,
        }
    }

    pub(crate) fn is_external_state_unknown(&self) -> bool {
        self.external_state_unknown
    }

    pub(crate) fn into_message(self) -> String {
        self.message
    }

    /// 保留错误类别、只改写文案（供上层追加诊断信息）。
    pub(crate) fn map_message(self, map: impl FnOnce(String) -> String) -> Self {
        Self {
            message: map(self.message),
            ..self
        }
    }
}

impl fmt::Display for McpCallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

pub(super) async fn execute(
    client: RunningService<RoleClient, ()>,
    call: CallToolRequestParams,
    timeout: Duration,
    cancel: Option<watch::Receiver<bool>>,
) -> Result<CallToolResult, McpCallError> {
    execute_with_settle_timeout(client, call, timeout, CANCEL_SETTLE_TIMEOUT, cancel).await
}

async fn execute_with_settle_timeout(
    client: RunningService<RoleClient, ()>,
    call: CallToolRequestParams,
    timeout: Duration,
    settle_timeout: Duration,
    cancel: Option<watch::Receiver<bool>>,
) -> Result<CallToolResult, McpCallError> {
    let result = async {
        if cancel.as_ref().is_some_and(|rx| *rx.borrow() || rx.has_changed().is_err()) {
            return Err(McpCallError::not_sent("MCP 调用已取消，未发送工具请求"));
        }
        // 共享预算已被初始化握手耗尽：不发注定超时的请求，按「未发送」处理
        // （调用方可重试），而不是发出去再标 external_state_unknown。
        if timeout.is_zero() {
            return Err(McpCallError::not_sent(
                "MCP 调用预算已被初始化耗尽，未发送工具请求",
            ));
        }
        let mut request = client.send_cancellable_request(
            ClientRequest::CallToolRequest(Request::new(call)),
            PeerRequestOptions::no_options(),
        ).await.map_err(|error| McpCallError::external_state_unknown(format!("external_state_unknown：MCP 请求发送失败：{error}")))?;
        let response = tokio::select! {
            response = &mut request.rx => Some(response),
            _ = tokio::time::sleep(timeout) => None,
            _ = cancelled(cancel) => None,
        };
        match response {
            Some(Ok(Ok(ServerResult::CallToolResult(result)))) => Ok(result),
            Some(Ok(Ok(other))) => Err(McpCallError::external_state_unknown(format!("MCP 返回了不合法的工具响应：{other:?}"))),
            Some(Ok(Err(error))) => Err(McpCallError::external_state_unknown(format!("external_state_unknown：MCP 调用失败：{error}"))),
            Some(Err(error)) => Err(McpCallError::external_state_unknown(format!("external_state_unknown：MCP 响应通道关闭：{error}"))),
            None => {
                // 取消通知限时：超时按「取消通知未能确认」继续走原错误路径。
                let notification = tokio::time::timeout(
                    settle_timeout,
                    request.cancel(Some("调用取消或超时".into())),
                )
                .await;
                Err(McpCallError::external_state_unknown(format!("external_state_unknown：MCP 调用取消或超时，无法确认外部操作停止，禁止自动重跑；取消通知结果：{notification:?}")))
            }
        }
    }.await;
    // 即使请求失败也要收敛 transport。保持 worker 与运行租约直到此处返回；
    // 收尾同样限时，挂死的 server 不得永久挂住 worker。
    let settled = tokio::time::timeout(settle_timeout, client.cancel()).await;
    let settle_note = match &settled {
        Ok(Ok(_)) => None,
        Ok(Err(error)) => Some(format!("MCP transport 收尾失败：{error}")),
        Err(_) => Some(format!(
            "MCP transport 收尾超时（{settle_timeout:?}）未确认收敛"
        )),
    };
    merge_settle_outcome(result, settle_note)
}

/// 收尾结果合入调用结果：成功结果优先（transport 收尾失败仅留痕，绝不覆盖
/// Ok）；失败结果保留原始错误与类别，收尾状态仅附加说明。
fn merge_settle_outcome(
    result: Result<CallToolResult, McpCallError>,
    settle_note: Option<String>,
) -> Result<CallToolResult, McpCallError> {
    match (result, settle_note) {
        (Ok(result), note) => {
            if let Some(note) = note {
                eprintln!("[mcp] 警告：{note}");
            }
            Ok(result)
        }
        (Err(error), None) => Err(error),
        (Err(error), Some(note)) => Err(error.map_message(|message| format!("{message}；{note}"))),
    }
}

async fn cancelled(cancel: Option<tokio::sync::watch::Receiver<bool>>) {
    let Some(mut rx) = cancel else {
        return std::future::pending().await;
    };
    while !*rx.borrow() {
        if rx.changed().await.is_err() {
            break;
        }
    }
}

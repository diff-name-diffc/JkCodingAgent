//! ACP 握手端到端测试的 stub 执行器（仅 `cargo test` 编译，不进应用产物）。
//!
//! 由 `agent::workflow::acp_exec::handshake_e2e` 单测经真实 `process::spawn`
//! 拉起，在 stdio 上以 v1 协议服务一整轮节点会话，复刻 claude-agent-acp
//! 的关键契约面：session/new 返回 modes + model 选择器、prompt 期间下发
//! session/update 通知并发起 session/request_permission、prompt 响应携带
//! `_meta.quota.token_count` 计费口径计数。收到的请求逐条追加到
//! `ACP_STUB_TRANSCRIPT` 指向的 JSONL（每条先落盘再应答——客户端看到应答
//! 时转录必然已可见），供测试侧断言客户端的线上行为。
//!
//! `ACP_STUB_MODE=crash` 时 initialize 回协议错误并写 stderr 标记，
//! 供失败路径测试（错误传播 + stderr 尾部诊断）。

use agent_client_protocol::schema::v1::{
    AgentCapabilities, ContentBlock, ContentChunk, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PermissionOption, PermissionOptionKind, PromptRequest,
    PromptResponse, RequestPermissionRequest, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigSelectOption, SessionMode, SessionModeState, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
    SetSessionModeResponse, StopReason, TextContent, ToolCall, ToolCallLocation, ToolCallStatus,
    ToolCallUpdate, ToolCallUpdateFields, UsageUpdate,
};
use agent_client_protocol::{Agent, Client, ConnectionTo, Responder, Stdio};
use serde_json::{json, Value};
use std::io::Write;
use std::sync::OnceLock;

/// 客户端 session/new 下发的 cwd：工具调用 locations 以工作区内绝对路径
/// 形态复刻真实执行器（映射层按此前缀归一为相对路径）。
static SESSION_CWD: OnceLock<String> = OnceLock::new();

/// 收到的请求转录到 JSONL：method + 序列化后的请求参数。
fn record<T: serde::Serialize>(method: &str, payload: &T) {
    let Some(path) = std::env::var_os("ACP_STUB_TRANSCRIPT") else {
        return;
    };
    let entry = json!({"method": method, "params": payload});
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let _ = writeln!(file, "{entry}");
}

/// 非请求事件（权限应答结果）转录。
fn record_value(entry: Value) {
    let Some(path) = std::env::var_os("ACP_STUB_TRANSCRIPT") else {
        return;
    };
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let _ = writeln!(file, "{entry}");
}

/// model 选择器：与 claude-agent-acp 的 configOptions 形态对齐（id/名称
/// 含 "model" 的选择器，客户端 apply_model 按此识别）。
fn model_config_options() -> Vec<SessionConfigOption> {
    vec![SessionConfigOption::select(
        "model",
        "Model",
        "default",
        vec![
            SessionConfigSelectOption::new("default", "Default"),
            SessionConfigSelectOption::new("sonnet", "Sonnet"),
            SessionConfigSelectOption::new("opus", "Opus"),
            SessionConfigSelectOption::new("haiku", "Haiku"),
        ],
    )
    .category(SessionConfigOptionCategory::Model)]
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let crash_mode = std::env::var("ACP_STUB_MODE").as_deref() == Ok("crash");
    Agent
        .builder()
        .name("acp-stub-agent")
        .on_receive_request(
            async move |request: InitializeRequest,
                        responder: Responder<InitializeResponse>,
                        _connection: ConnectionTo<Client>| {
                record("initialize", &request);
                if crash_mode {
                    eprintln!("stub initialize 故意失败（ACP_STUB_MODE=crash）");
                    return responder.respond_with_internal_error("stub 初始化失败");
                }
                responder.respond(
                    InitializeResponse::new(request.protocol_version)
                        .agent_capabilities(AgentCapabilities::new()),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: NewSessionRequest,
                        responder: Responder<NewSessionResponse>,
                        _connection: ConnectionTo<Client>| {
                record("session/new", &request);
                let _ = SESSION_CWD.set(request.cwd.display().to_string());
                responder.respond(
                    NewSessionResponse::new("stub-session")
                        .modes(SessionModeState::new(
                            "default",
                            vec![
                                SessionMode::new("default", "Default"),
                                SessionMode::new("bypassPermissions", "Bypass Permissions"),
                                SessionMode::new("plan", "Plan Mode"),
                            ],
                        ))
                        .config_options(model_config_options()),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: SetSessionModeRequest,
                        responder: Responder<SetSessionModeResponse>,
                        _connection: ConnectionTo<Client>| {
                record("session/set_mode", &request);
                responder.respond(SetSessionModeResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: SetSessionConfigOptionRequest,
                        responder: Responder<SetSessionConfigOptionResponse>,
                        _connection: ConnectionTo<Client>| {
                record("session/set_config_option", &request);
                responder.respond(SetSessionConfigOptionResponse::new(model_config_options()))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PromptRequest,
                        responder: Responder<PromptResponse>,
                        connection: ConnectionTo<Client>| {
                let prompt_text = request
                    .prompt
                    .iter()
                    .find_map(|block| match block {
                        ContentBlock::Text(text) => Some(text.text.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                record(
                    "session/prompt",
                    &json!({"sessionId": request.session_id, "promptText": prompt_text}),
                );
                let session_id = request.session_id.clone();
                let cwd = SESSION_CWD.get().cloned().unwrap_or_default();
                // 通知链：消息增量 → 工具调用（工作区内 locations，供映射层
                // 记录受影响文件）→ 工具完成 → 用量。
                let _ = connection.send_notification(SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                        TextContent::new("stub 产出"),
                    ))),
                ));
                let _ = connection.send_notification(SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::ToolCall(
                        ToolCall::new("call-1", "读取文件")
                            .name("Read")
                            .status(ToolCallStatus::InProgress)
                            .raw_input(json!({"path": "src/main.rs"}))
                            .locations(vec![ToolCallLocation::new(format!("{cwd}/src/main.rs"))]),
                    ),
                ));
                let _ = connection.send_notification(SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                        "call-1",
                        ToolCallUpdateFields::new()
                            .status(ToolCallStatus::Completed)
                            .raw_output(json!("ok")),
                    )),
                ));
                let _ = connection.send_notification(SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::UsageUpdate(UsageUpdate::new(35_450, 200_000)),
                ));
                // 权限请求（EnterPlanMode 走客户端静态放行路径）：与被测客户端
                // 的 permission 应答同构——prompt 处理器立即返回，请求与回合
                // 应答挪进独立任务（处理器内同步等待反向应答会卡死事件循环）。
                tokio::spawn(async move {
                    let permission = RequestPermissionRequest::new(
                        session_id.clone(),
                        ToolCallUpdate::new(
                            "call-2",
                            ToolCallUpdateFields::new()
                                .name("EnterPlanMode")
                                .title("进入计划模式"),
                        ),
                        vec![
                            PermissionOption::new(
                                "once",
                                "Yes, enter plan mode",
                                PermissionOptionKind::AllowOnce,
                            ),
                            PermissionOption::new(
                                "reject",
                                "No, start implementing",
                                PermissionOptionKind::RejectOnce,
                            ),
                        ],
                    );
                    let response = match connection.send_request(permission).block_task().await {
                        Ok(response) => response,
                        Err(error) => {
                            record_value(json!({
                                "method": "session/request_permission.error",
                                "error": format!("{error:?}")
                            }));
                            return;
                        }
                    };
                    record_value(json!({
                        "method": "session/request_permission.outcome",
                        "outcome": format!("{:?}", response.outcome),
                    }));
                    let mut meta = serde_json::Map::new();
                    meta.insert(
                        "quota".into(),
                        json!({
                            "token_count": {
                                "totalTokens": 35_450,
                                "inputTokens": 1_200,
                                "cachedInputTokens": 30_000,
                                "cachedWriteTokens": 4_000,
                                "outputTokens": 250,
                            }
                        }),
                    );
                    let _ = responder.respond(PromptResponse::new(StopReason::EndTurn).meta(meta));
                });
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_to(Stdio::new())
        .await?;
    Ok(())
}

//! claude-agent-acp 子进程与 ACP 连接的生命周期。
//!
//! 子进程由 `process.rs` 自有 spawn（env_clear + 白名单 env + 进程组守卫 +
//! stdout 单行上限），传输层用 crate 的 `ByteStreams` 接手动接管的 stdio。
//! 进程回收由 `process::ChildGuard` 负责：连接 future 被 drop（取消/超时/
//! 结束）时守卫随作用域析构，SIGKILL 整个进程组（含 node 派生的孙进程）。
//! 因此取消路径只需「发 session/cancel（尽力而为）+ 结束函数作用域」。

use std::path::PathBuf;
use std::sync::Arc;

use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, Error, InitializeRequest, NewSessionRequest,
    NewSessionResponse, PermissionOptionKind, PromptRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SessionConfigKind,
    SessionConfigOptionValue, SessionConfigSelectOptions, SessionId, SessionNotification,
    SetSessionConfigOptionRequest, SetSessionModeRequest, StopReason, TextContent,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::ByteStreams;
use agent_client_protocol::{Agent, ConnectionTo};
use parking_lot::Mutex;
use tokio::sync::watch;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use super::launcher::LaunchPlan;
use super::{process, HandlerContext};
use crate::agent::graph::harness::{PermissionMode, ResolvedNodeHarness};

/// JSON-RPC 应用层错误码（-32603 internal error 区间），仅用于区分错误来源。
const ERR_INITIALIZE: i32 = -32001;
const ERR_SESSION_NEW: i32 = -32002;
const ERR_PROMPT: i32 = -32003;
const ERR_SET_MODE: i32 = -32004;

/// 会话级失败：取消与错误分开，调用方据此结算节点终态。
#[derive(Debug)]
pub(super) enum AcpSessionError {
    Cancelled,
    Failed(String),
}

pub(super) struct PromptTurnResult {
    pub stop_reason: StopReason,
    pub diagnostics: Vec<String>,
}

/// 单轮提示会话：spawn（env_clear + 白名单 env）→ initialize → session/new →
/// 设模式/模型 → session/prompt 直到整轮结束。取消时发 session/cancel（尽力
/// 而为）后结束作用域——进程组守卫 drop 时 SIGKILL 整组。
pub(super) async fn run_prompt_turn(
    plan: LaunchPlan,
    cwd: PathBuf,
    prompt: String,
    harness: &ResolvedNodeHarness,
    handler: HandlerContext,
    mut cancel_rx: watch::Receiver<bool>,
) -> Result<PromptTurnResult, AcpSessionError> {
    let spawned = process::spawn(&plan).map_err(AcpSessionError::Failed)?;
    // 守卫持有至函数结束：任何返回路径（成功/失败/取消/超时 drop）都会
    // 触发进程组回收。
    let _guard = spawned.guard;
    let (stderr_tail, _stderr_task) = process::drain_stderr(spawned.stderr);
    let transport = ByteStreams::new(
        spawned.stdin.compat_write(),
        process::BoundedLineReader::new(spawned.stdout.compat()),
    );

    // 连接/会话句柄共享槽：取消分支用它们发 session/cancel。
    let conn_slot: Arc<Mutex<Option<ConnectionTo<Agent>>>> = Arc::new(Mutex::new(None));
    let session_slot: Arc<Mutex<Option<SessionId>>> = Arc::new(Mutex::new(None));

    let mut diagnostics = plan.diagnostics;
    let permission_mode = harness.permission_mode;
    let mode_id = permission_mode.mode_id();
    let plan_mode = matches!(permission_mode, PermissionMode::Plan);
    let model_keyword = harness.model_id.clone();

    let notify_handler = handler.clone();
    let permission_handler = handler.clone();
    // 计划批准后的 bypass 跟进需要连接/会话句柄（与取消分支共享同一对槽）。
    let permission_conn_slot = Arc::clone(&conn_slot);
    let permission_session_slot = Arc::clone(&session_slot);
    let connect = agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                // 处理器跑在连接事件循环上：只做同步映射 + 事件广播/入队，
                // 任何阻塞都会卡住后续消息（crate 文档明确警告）。
                notify_handler.handle_update(&notification.update);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _connection| {
                // 权限应答可能要等全局审查模型（秒级 LLM）：连接事件循环是
                // 单异步任务，handler 运行期间不处理任何其他消息（crate 文档
                // 明确警告）。把应答整体挪进独立任务、经 responder 延迟应答
                // （恰好应答一次的语义由 Responder 保证；任务被丢弃时 drop
                // guard 会补 Internal Error，不会让请求悬挂）。
                let handler = permission_handler.clone();
                let conn_slot = Arc::clone(&permission_conn_slot);
                let session_slot = Arc::clone(&permission_session_slot);
                tokio::spawn(async move {
                    let is_plan_approval = request
                        .tool_call
                        .fields
                        .name
                        .as_deref()
                        .map(|name| name == "ExitPlanMode")
                        .unwrap_or(false);
                    let outcome = handler.answer_permission(&request).await;
                    // 「批准」= 选中了一个 allow 类选项。Selected 本身不够：
                    // 选项表缺 allow 变体时 `select_plan_approval` 会退到
                    // reject_once（Selected），此时切 bypass 等于推翻拒绝。
                    let approved = match &outcome {
                        RequestPermissionOutcome::Selected(selected) => {
                            request.options.iter().any(|option| {
                                option.option_id == selected.option_id
                                    && matches!(
                                        option.kind,
                                        PermissionOptionKind::AllowOnce
                                            | PermissionOptionKind::AllowAlways
                                    )
                            })
                        }
                        _ => false,
                    };
                    if let Err(error) = responder.respond(RequestPermissionResponse::new(outcome)) {
                        eprintln!(
                            "[graph] ACP 权限应答发送失败（节点 {}）：{error}",
                            handler.node_id
                        );
                        return;
                    }
                    // 计划批准后统一切回 bypassPermissions：0.79.0 的
                    // ExitPlanMode 选项表没有 bypass 变体（elevated 档恒为
                    // auto），批准选项只充当载体，目标级别由此处保证。
                    if is_plan_approval && approved {
                        restore_bypass_after_plan_approval(&conn_slot, &session_slot, &handler)
                            .await;
                    }
                });
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(transport, {
            let conn_slot = Arc::clone(&conn_slot);
            let session_slot = Arc::clone(&session_slot);
            async move |connection: ConnectionTo<Agent>| {
                *conn_slot.lock() = Some(connection.clone());
                connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await
                    .map_err(|error| {
                        Error::new(ERR_INITIALIZE, format!("ACP initialize 失败：{error}"))
                    })?;
                let session = connection
                    .send_request(NewSessionRequest::new(cwd))
                    .block_task()
                    .await
                    .map_err(|error| {
                        Error::new(ERR_SESSION_NEW, format!("ACP 创建会话失败：{error}"))
                    })?;
                let session_id = session.session_id.clone();
                *session_slot.lock() = Some(session_id.clone());
                let mut stage_diagnostics = Vec::new();
                if let Err(error) = apply_mode(&connection, &session, &session_id, mode_id).await {
                    if plan_mode {
                        // plan 节点 fail-closed：模式未生效意味着「先计划后执行」
                        // 的节点语义落空，直接失败优于带病运行。bypass 节点设置
                        // 失败仅记诊断（宽松优先，浮出的权限请求仍由审查 AI
                        // 把关）。
                        return Err(Error::new(
                            ERR_SET_MODE,
                            format!("plan 节点权限模式未能生效（fail-closed）：{error}"),
                        ));
                    }
                    stage_diagnostics.push(format!("{error}，沿用当前模式"));
                }
                apply_model(
                    &connection,
                    &session,
                    &session_id,
                    &model_keyword,
                    &mut stage_diagnostics,
                )
                .await;
                let response = connection
                    .send_request(PromptRequest::new(
                        session_id,
                        vec![ContentBlock::Text(TextContent::new(prompt))],
                    ))
                    .block_task()
                    .await
                    .map_err(|error| {
                        Error::new(ERR_PROMPT, format!("ACP 执行提示失败：{error}"))
                    })?;
                Ok(PromptTurnResult {
                    stop_reason: response.stop_reason,
                    diagnostics: stage_diagnostics,
                })
            }
        });

    tokio::pin!(connect);
    tokio::select! {
        biased;
        _ = wait_for_cancel(&mut cancel_rx) => {
            // 先发 session/cancel（尽力而为）；本分支返回后连接 future 与
            // 进程守卫随作用域析构——守卫 SIGKILL 整个进程组。
            let connection = conn_slot.lock().clone();
            let session_id = session_slot.lock().clone();
            if let (Some(connection), Some(session_id)) = (connection, session_id) {
                let _ = connection.send_notification(CancelNotification::new(session_id));
            }
            Err(AcpSessionError::Cancelled)
        }
        result = &mut connect => match result {
            Ok(mut turn) => {
                diagnostics.append(&mut turn.diagnostics);
                turn.diagnostics = diagnostics;
                Ok(turn)
            }
            Err(error) => {
                // 失败时附上执行器 stderr 尾部（启动崩溃/协议错误的主要线索）。
                let stderr = stderr_tail.take_string();
                let stderr_note = if stderr.is_empty() {
                    String::new()
                } else {
                    format!("\n执行器 stderr 尾部：\n{stderr}")
                };
                Err(AcpSessionError::Failed(format!(
                    "{error}{stderr_note}\n（如为启动失败，请检查本机 Node.js ≥ 22 与设置中的 ACP 启动命令）"
                )))
            }
        },
    }
}

/// 与 runner::wait_for_cancel 同语义：信号置位或发送端被丢弃（fail-closed）。
async fn wait_for_cancel(cancel_rx: &mut watch::Receiver<bool>) {
    while !*cancel_rx.borrow() {
        if cancel_rx.changed().await.is_err() {
            return;
        }
    }
}

/// 计划批准后的 bypass 跟进：把会话权限模式切回 bypassPermissions。
/// 0.79.0 的 ExitPlanMode 批准选项没有 bypass 变体（elevated 档恒为 auto），
/// 目标级别只能由客户端在批准后主动保证。失败不致命（记审计活动 + 日志，
/// 会话停留在批准选项携带的模式继续运行）。
async fn restore_bypass_after_plan_approval(
    conn_slot: &Arc<Mutex<Option<ConnectionTo<Agent>>>>,
    session_slot: &Arc<Mutex<Option<SessionId>>>,
    handler: &super::HandlerContext,
) {
    let Some(connection) = conn_slot.lock().clone() else {
        return;
    };
    let Some(session_id) = session_slot.lock().clone() else {
        return;
    };
    let result = connection
        .send_request(SetSessionModeRequest::new(
            session_id,
            "bypassPermissions".to_string(),
        ))
        .block_task()
        .await;
    let (title, content) = match &result {
        Ok(_) => (
            "权限模式：计划批准后切换 bypassPermissions".to_string(),
            "执行计划已获批准，会话切回 bypassPermissions 全权限模式".to_string(),
        ),
        Err(error) => (
            "权限模式：计划批准后切换 bypassPermissions 失败".to_string(),
            format!("会话沿用批准选项携带的模式继续运行：{error}"),
        ),
    };
    let action = handler.mapper.lock().lifecycle_activity(&title, &content);
    handler.apply(vec![action]);
    if result.is_err() {
        eprintln!("[graph] {title}（节点 {}）：{content}", handler.node_id);
    }
}

/// 会话权限模式应用：usePlanMode → plan（先计划后执行），否则
/// bypassPermissions（全权限）。返回 Err 表示模式未能生效；调用方决定
/// fail-closed（plan 节点：计划先行是节点语义）还是降级继续（bypass 节点
/// 记 diagnostic 沿用当前模式，浮出的权限请求仍由审查 AI 把关）。
async fn apply_mode(
    connection: &ConnectionTo<Agent>,
    session: &NewSessionResponse,
    session_id: &SessionId,
    want: &str,
) -> Result<(), String> {
    let Some(modes) = &session.modes else {
        return Err("会话未返回权限模式列表，无法确认执行器权限模式".into());
    };
    if modes.current_mode_id.0.as_ref() == want {
        return Ok(());
    }
    if !modes
        .available_modes
        .iter()
        .any(|mode| mode.id.0.as_ref() == want)
    {
        return Err(format!(
            "执行器不支持权限模式 '{want}'（可用：{}），当前模式 '{}'",
            modes
                .available_modes
                .iter()
                .map(|mode| mode.id.0.as_ref())
                .collect::<Vec<_>>()
                .join(" / "),
            modes.current_mode_id.0,
        ));
    }
    connection
        .send_request(SetSessionModeRequest::new(
            session_id.clone(),
            want.to_string(),
        ))
        .block_task()
        .await
        .map_err(|error| format!("设置权限模式 '{want}' 失败：{error}"))?;
    Ok(())
}

/// 会话模型：经 configOptions 中 id/名称含 "model" 的选择器设置。
/// `default` 不设置（继承 Claude 登录态默认模型）；找不到选项或设置失败时
/// 记 diagnostic 并沿用当前模型（不 fail）。
async fn apply_model(
    connection: &ConnectionTo<Agent>,
    session: &NewSessionResponse,
    session_id: &SessionId,
    keyword: &str,
    diagnostics: &mut Vec<String>,
) {
    if keyword.is_empty() || keyword == "default" {
        return;
    }
    let Some(config_options) = &session.config_options else {
        diagnostics.push(format!(
            "会话未返回配置项列表，无法设置模型 '{keyword}'，沿用默认模型"
        ));
        return;
    };
    let Some(option) = config_options.iter().find(|option| {
        option.id.0.as_ref() == "model" || option.name.to_ascii_lowercase().contains("model")
    }) else {
        diagnostics.push(format!(
            "会话配置项中没有模型选择器，无法设置模型 '{keyword}'，沿用默认模型"
        ));
        return;
    };
    let SessionConfigKind::Select(select) = &option.kind else {
        diagnostics.push("模型配置项不是选择器，沿用默认模型".into());
        return;
    };
    let values: Vec<String> = match &select.options {
        SessionConfigSelectOptions::Ungrouped(options) => options
            .iter()
            .map(|item| item.value.0.to_string())
            .collect(),
        SessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .flat_map(|group| group.options.iter().map(|item| item.value.0.to_string()))
            .collect(),
        _ => Vec::new(),
    };
    let lowered = keyword.to_ascii_lowercase();
    let Some(target) = values
        .iter()
        .find(|value| value.as_str() == keyword)
        .or_else(|| {
            values
                .iter()
                .find(|value| value.to_ascii_lowercase().contains(&lowered))
        })
        .cloned()
    else {
        diagnostics.push(format!(
            "模型配置项中没有 '{keyword}'（可选：{}），沿用当前模型",
            values.join(", ")
        ));
        return;
    };
    if let Err(error) = connection
        .send_request(SetSessionConfigOptionRequest::new(
            session_id.clone(),
            option.id.clone(),
            SessionConfigOptionValue::value_id(target),
        ))
        .block_task()
        .await
    {
        diagnostics.push(format!("设置模型 '{keyword}' 失败：{error}，沿用当前模型"));
    }
}

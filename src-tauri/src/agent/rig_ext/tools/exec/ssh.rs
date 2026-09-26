//! SSH 工具组：`ssh_list_servers` / `ssh_exec`。
//! 迁移自旧自实现工具层（已随迁移删除）；连接池复用 `crate::ssh_tool::SshSessionManager`。

use serde_json::{json, Value};

use super::super::common::{
    string_arg, u64_arg, with_compression_parameters, COMMAND_FORCE_COMPRESS_AFTER_CHARS,
};
use crate::agent::command_history::{self, CommandHistoryStatus};
use crate::agent::db::DispatcherDb;
use crate::ssh_tool::{CommandFailure, CommandFailureKind, SshSessionManager};
use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use std::path::PathBuf;

#[cfg(test)]
mod tests;

pub(super) fn ssh_tools(
    manager: SshSessionManager,
    workspace: PathBuf,
    workspace_id: String,
    db: DispatcherDb,
    review_context: crate::agent::rig_ext::review::RigReviewContext,
) -> Vec<PortableDynamicTool> {
    vec![
        ssh_list_servers_tool(manager.clone()),
        ssh_exec_tool(manager, workspace, workspace_id, db, review_context),
    ]
}

fn ssh_list_servers_tool(manager: SshSessionManager) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "ssh_list_servers",
        "列出已启用的 SSH 服务器（全局配置，所有项目共享）。只返回 server_id、名称、描述和标签，不暴露 IP、端口、账号或密码。",
        json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
        move |_args| {
            let manager = manager.clone();
            Box::pin(async move {
                let text = match manager.list_servers_async().await {
                    Ok(servers) => {
                        if servers.is_empty() {
                            "没有已启用的 SSH server。请先在 Aha 智能体设置中配置 SSH 工具。"
                                .to_string()
                        } else {
                            match serde_json::to_string_pretty(&json!({ "servers": servers })) {
                                Ok(text) => text,
                                Err(error) => {
                                    format!("错误：序列化 SSH server 列表失败：{error}")
                                }
                            }
                        }
                    }
                    Err(error) => format!("错误：读取 SSH server 列表失败：{error}"),
                };
                Ok(ToolOutput::text(text))
            })
        },
    )
}

/// SSH 命令执行工具。
///
/// 并发语义：同一 `server_id + session_id` 的并发调用复用同一条 SSH 连接，
/// 并发命令各走独立 channel（协议级隔离，输出不交错）；跨 session_id 的调用互不阻塞。
fn ssh_exec_tool(
    manager: SshSessionManager,
    workspace: PathBuf,
    workspace_id: String,
    db: DispatcherDb,
    review_context: crate::agent::rig_ext::review::RigReviewContext,
) -> PortableDynamicTool {
    let parameters = with_compression_parameters(
        json!({
            "type": "object",
            "properties": {
                "server_id": {
                    "type": "string",
                    "description": "ssh_list_servers 返回的服务器 id"
                },
                "session_id": {
                    "type": "string",
                    "description": "会话 id。相同 server_id + session_id 会复用 SSH 连接；建议使用当前任务或排障主题的稳定短 id"
                },
                "command": {
                    "type": "string",
                    "description": "要在远程服务器执行的非交互式 shell 命令。禁止依赖交互输入（密码提示、y/n 确认、分页器、REPL）；这类命令会被检测到并中止。"
                },
                "stdin": {
                    "type": "string",
                    "description": "可选。命令启动后写入其标准输入的内容，随后关闭输入端，上限 32000 字符（与安全审查送审上限一致，超出会被拒绝执行）。用于 here-doc / 管道喂入场景（如向读取 stdin 的程序提供内容）。stdin 内容会随命令一起提交安全审查，禁止用 stdin 携带未审查的破坏性内容。不可用于回应交互式密码/确认提示——这类场景请改写为非交互命令。"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "本次命令超时时间，单位秒，默认使用服务器配置",
                    "minimum": 1,
                    "maximum": 300
                }
            },
            "required": ["server_id", "session_id", "command"]
        }),
        false,
        COMMAND_FORCE_COMPRESS_AFTER_CHARS,
        "默认直接返回原始输出：8000 字符内全量内联，超出截断且完整原文保留在工具产物中。报错原文、状态值、配置片段等需要逐字核对的内容应保持 compress=false。仅当预期输出很大（全量日志、大目录清单等）且只需确认特定信息时才设 compress=true，并在 compress_intent 中写明要确认什么。",
    );
    PortableDynamicTool::new(
        "ssh_exec",
        "在指定 SSH server 上执行单个非交互式命令，返回 stdout/stderr/退出码。复用 session_id 对应的 SSH 连接，但每次调用是一次独立命令（不保留 cd、环境变量等 shell 状态）。\n\
重要约束：命令必须是非交互式的——不得弹出密码、y/n 确认、分页器或进入 REPL。若命令疑似在等待输入，工具会主动中止并报错：输出末尾命中密码/确认等提示符时最快 8 秒中止；完全静默的命令（如 sleep、慢查询、写文件的备份）按 timeout_secs 放宽静默容忍（最长 60 秒）。改用非交互等价形式：sudo 用免密账号或 NOPASSWD；包管理/删除加 -y/--yes；分页器设 PAGER=cat、GIT_PAGER=cat；mysql/psql 用 -e/-c；需要向命令喂内容时用 stdin 参数（here-doc），不要指望终端回应交互提示。",
        parameters,
        move |args| {
            let manager = manager.clone();
            let workspace = workspace.clone();
            let workspace_id = workspace_id.clone();
            let db = db.clone();
            let review_context = review_context.clone();
            // 取消信号在本层（唯一处于 agent 循环 task-local 作用域的边界）读取
            // 一次后向下显式传递；无 task-local（如脱离循环的调用）时为 None，
            // 传输层按「无取消源」处理，不误判为已取消。
            let cancel_rx =
                crate::agent::rig_ext::r#loop::invocation::ToolInvocationContext::current()
                    .map(|context| context.cancel_rx);
            Box::pin(async move {
                ssh_exec_text(
                    &args,
                    manager,
                    workspace,
                    workspace_id,
                    db,
                    cancel_rx,
                    review_context,
                )
                .await
                .map(ToolOutput::text)
            })
        },
    )
}

async fn ssh_exec_text(
    args: &Value,
    manager: SshSessionManager,
    workspace: PathBuf,
    workspace_id: String,
    db: DispatcherDb,
    cancel_rx: Option<tokio::sync::watch::Receiver<bool>>,
    review_context: crate::agent::rig_ext::review::RigReviewContext,
) -> Result<String, ToolExecutionError> {
    let Some(server_id) = string_arg(args, "server_id") else {
        return Err(ToolExecutionError::invalid_args(
            "错误：缺少必填参数 server_id；请先调用 ssh_list_servers。".to_string(),
        ));
    };
    let Some(session_id) = string_arg(args, "session_id") else {
        return Err(ToolExecutionError::invalid_args(
            "错误：缺少必填参数 session_id。".to_string(),
        ));
    };
    let Some(command) = string_arg(args, "command") else {
        return Err(ToolExecutionError::invalid_args(
            "错误：缺少必填参数 command。".to_string(),
        ));
    };
    let stdin = string_arg(args, "stdin");

    // 审计元数据需要会话标题（审查阻断记录与执行记录共用）。读取失败仅降级
    // 审计展示（标题留空），仅留日志，不应中止命令执行（对齐 sync_directory）。
    let session_title = db
        .get_session_title_async(&workspace_id)
        .await
        .unwrap_or_else(|error| {
            eprintln!("[ssh-tool] 读取会话标题失败，审计记录标题留空：{error}");
            String::new()
        });

    // 安全审查门禁：fail-closed，且是 ssh_exec 的唯一审查层。
    // - 未配置审查模型：默认拦截可执行命令，不得跳过审查放行。
    // - 已配置且服务器开启审查：执行前评估命令（含 stdin）安全性。
    // - 审查异常或判定不通过：拦截并写入审计，不执行命令。
    // - 服务器显式关闭「执行前审查」开关：按配置放行（设计内的豁免通道）。
    let review_outcome: Option<crate::ssh_tool::SshAuditReview> = match review_context
        .config
        .as_ref()
    {
        None => {
            // 与 review-denied 路径一致：未配置审查的阻断也登记命令台账。
            command_history::record(
                &workspace_id,
                "ssh_exec",
                &server_id,
                &command,
                CommandHistoryStatus::Blocked,
                "未配置安全审查，无法评估命令安全性",
            );
            let blocked = crate::ssh_tool::SshAuditReview {
                allowed: false,
                reason: "未配置安全审查，无法评估命令安全性".to_string(),
            };
            if let Ok(record) = manager
                .record_review_blocked(
                    workspace.clone(),
                    workspace_id.clone(),
                    session_title.clone(),
                    server_id.clone(),
                    session_id.clone(),
                    command.clone(),
                    blocked,
                )
                .await
            {
                return Err(ToolExecutionError::refused(format!(
                    "错误：未配置安全审查，已拒绝执行命令。请先在应用设置中配置安全审查模型。\n\n{}",
                    crate::ssh_tool::render_ssh_audit_record_markdown(&record)
                )));
            }
            return Err(ToolExecutionError::refused(
                "错误：未配置安全审查，已拒绝执行命令。请先在应用设置中配置安全审查模型。"
                    .to_string(),
            ));
        }
        Some(review_config) => match manager.server_config_async(server_id.clone()).await {
            Ok(server) if server.review_enabled => {
                let payload = review_context.build_payload(
                    &workspace_id,
                    Some(args),
                    crate::agent::ssh_review::CommandReviewTarget::Ssh(
                        crate::agent::ssh_review::SshReviewServerInfo {
                            id: server.id.clone(),
                            description: server.description.clone(),
                            host: server.host.clone(),
                            port: server.port,
                            username: server.username.clone(),
                            tags: server.tags.clone(),
                        },
                    ),
                    command.clone(),
                    stdin.clone(),
                );
                match crate::agent::ssh_review::review_shell_command(review_config, &payload).await
                {
                    Ok(verdict) => Some(crate::ssh_tool::SshAuditReview {
                        allowed: verdict.allowed,
                        reason: verdict.reason,
                    }),
                    Err(error) => {
                        // 与 review-denied 路径一致：审查异常导致的阻断也登记台账。
                        command_history::record(
                            &workspace_id,
                            "ssh_exec",
                            &server_id,
                            &command,
                            CommandHistoryStatus::Blocked,
                            &format!("审查服务异常：{error}"),
                        );
                        let blocked = crate::ssh_tool::SshAuditReview {
                            allowed: false,
                            reason: format!("审查服务异常：{error}"),
                        };
                        let record_result = manager
                            .record_review_blocked(
                                workspace.clone(),
                                workspace_id.clone(),
                                session_title.clone(),
                                server_id.clone(),
                                session_id.clone(),
                                command.clone(),
                                blocked,
                            )
                            .await;
                        if let Ok(record) = record_result {
                            return Err(ToolExecutionError::refused(format!(
                                "错误：命令已被安全审查拦截（审查服务异常：{error}）。\n\n{}",
                                crate::ssh_tool::render_ssh_audit_record_markdown(&record)
                            )));
                        }
                        return Err(ToolExecutionError::refused(format!(
                            "错误：命令已被安全审查拦截（审查服务异常：{error}）。如需放行，可在 SSH 工具配置中关闭该服务器的「执行前审查」开关。"
                        )));
                    }
                }
            }
            Ok(_) => None,
            Err(error) => return Err(ToolExecutionError::other(format!("错误：{error}"))),
        },
    };

    // 判定为不通过：写入「被拦截」审计记录并阻断，同时登记命令台账
    //（供后续命令的安全审查判断来龙去脉）。
    if let Some(review) = &review_outcome {
        if !review.allowed {
            let reason = review.reason.clone();
            command_history::record(
                &workspace_id,
                "ssh_exec",
                &server_id,
                &command,
                CommandHistoryStatus::Blocked,
                &reason,
            );
            let record_result = manager
                .record_review_blocked(
                    workspace.clone(),
                    workspace_id.clone(),
                    session_title.clone(),
                    server_id.clone(),
                    session_id.clone(),
                    command.clone(),
                    review.clone(),
                )
                .await;
            if let Ok(record) = record_result {
                return Err(ToolExecutionError::refused(
                    crate::agent::ssh_review::with_confirm_guidance(
                        crate::ssh_tool::render_ssh_audit_record_markdown(&record),
                        &reason,
                    ),
                ));
            }
            return Err(ToolExecutionError::refused(crate::agent::ssh_review::with_confirm_guidance(
                format!(
                    "错误：命令已被安全审查拦截：{reason}。如需放行，可在 SSH 工具配置中关闭该服务器的「执行前审查」开关。"
                ),
                &reason,
            )));
        }
    }

    // 台账登记需要命令与目标标识，而下方 execute 会按值消费它们，先克隆留存。
    let history_server_id = server_id.clone();
    let history_command = command.clone();
    match manager
        .execute(
            workspace,
            workspace_id.clone(),
            session_title,
            server_id,
            session_id,
            command,
            stdin,
            u64_arg(args, "timeout_secs"),
            cancel_rx,
            review_outcome,
        )
        .await
    {
        Ok(result) => {
            let rendered = serde_json::to_string_pretty(&result).map_err(|error| {
                ToolExecutionError::other(format!("错误：序列化 SSH 执行结果失败：{error}"))
            })?;
            command_history::record(
                &workspace_id,
                "ssh_exec",
                &history_server_id,
                &history_command,
                CommandHistoryStatus::Executed,
                &rendered,
            );
            classify_ssh_result(&result, rendered)
        }
        Err(failure) => Err(map_command_failure(failure)),
    }
}

// manager.execute 的 Err 携带结构化 kind，按语义映射：
// - 发送前取消 → cancelled（远端状态确定未改变，台账记取消而非可恢复错误）；
// - 执行请求失败 → external_state_unknown + 禁止自动重跑（app_policy 按 code 匹配）；
// - 其余（配置 / 校验 / 建链）→ 普通执行失败。
fn map_command_failure(failure: CommandFailure) -> ToolExecutionError {
    match failure.kind {
        CommandFailureKind::CancelledNotSent => {
            ToolExecutionError::cancelled(format!("错误：{}", failure.message))
        }
        CommandFailureKind::ExternalStateUnknown => {
            ToolExecutionError::other(format!("错误：SSH 命令执行失败：{}", failure.message))
                .with_code("external_state_unknown")
                .with_retryable(false)
        }
        CommandFailureKind::Other => {
            ToolExecutionError::other(format!("错误：SSH 命令执行失败：{}", failure.message))
        }
    }
}

// 输出捕获完成不等于命令成功；完整 JSON（包括退出码）仍进入产物。
fn classify_ssh_result(
    result: &crate::ssh_tool::SshExecResult,
    rendered: String,
) -> Result<String, ToolExecutionError> {
    if result.cancelled {
        Err(ToolExecutionError::cancelled(rendered)
            .with_code("external_state_unknown")
            .with_retryable(false))
    } else if result.interactive_blocked {
        // 交互式命令被中止（command_exec 会同时置 external_state_unknown=true，
        // 必须先于此分支判断）：工具描述引导模型改写为非交互命令后重试，
        // 与「结果未知、禁止自动重跑」语义不同，归入 command_failed。
        Err(ToolExecutionError::other(rendered).with_code("command_failed"))
    } else if result.external_state_unknown {
        Err(ToolExecutionError::other(rendered)
            .with_code("external_state_unknown")
            .with_retryable(false))
    } else if result.exit_code != 0 {
        Err(ToolExecutionError::other(rendered).with_code("command_failed"))
    } else {
        Ok(rendered)
    }
}

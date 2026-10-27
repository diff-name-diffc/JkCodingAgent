use std::path::Path;

use rig::tool::ToolExecutionError;
use serde_json::Value;
use tauri::AppHandle;
use tokio::sync::watch;

use crate::agent::command_history::{self, CommandHistoryStatus};
use crate::agent::rig_ext::review::RigReviewContext;
use crate::ssh_tool::{SshAuditReview, SshSessionManager};

pub(crate) struct SshCommandGateRequest<'a> {
    pub manager: &'a SshSessionManager,
    pub workspace: &'a Path,
    pub workspace_id: &'a str,
    pub session_title: &'a str,
    pub server_id: &'a str,
    pub session_id: &'a str,
    pub tool_name: &'a str,
    pub command: &'a str,
    pub stdin: Option<String>,
    pub elevate: bool,
    pub args: &'a Value,
    pub review_context: &'a RigReviewContext,
    pub app_handle: Option<AppHandle>,
    pub cancel_rx: Option<watch::Receiver<bool>>,
}

pub(crate) async fn authorize_ssh_command(
    request: SshCommandGateRequest<'_>,
) -> Result<Option<SshAuditReview>, ToolExecutionError> {
    let SshCommandGateRequest {
        manager,
        workspace,
        workspace_id,
        session_title,
        server_id,
        session_id,
        tool_name,
        command,
        stdin,
        elevate,
        args,
        review_context,
        app_handle,
        cancel_rx,
    } = request;
    let mut review_outcome: Option<SshAuditReview> = match review_context.config.as_ref() {
        None => {
            command_history::record(
                workspace_id,
                tool_name,
                server_id,
                command,
                CommandHistoryStatus::Blocked,
                "未配置安全审查，无法评估命令安全性",
            );
            let blocked = SshAuditReview {
                allowed: false,
                user_approved: false,
                reason: "未配置安全审查，无法评估命令安全性".to_string(),
            };
            if let Ok(record) = manager
                .record_review_blocked(
                    workspace.to_path_buf(),
                    workspace_id.to_string(),
                    session_title.to_string(),
                    server_id.to_string(),
                    session_id.to_string(),
                    command.to_string(),
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
        Some(review_config) => match manager.server_config_async(server_id.to_string()).await {
            Ok(server) if server.review_enabled => {
                let payload = review_context.build_payload(
                    workspace_id,
                    Some(args),
                    crate::agent::ssh_review::CommandReviewTarget::Ssh(
                        crate::agent::ssh_review::SshReviewServerInfo {
                            id: server.id.clone(),
                            description: server.description.clone(),
                            host: server.host.clone(),
                            port: server.port,
                            username: server.username.clone(),
                            tags: server.tags.clone(),
                            elevated: elevate,
                        },
                    ),
                    command.to_string(),
                    stdin.clone(),
                );
                match crate::agent::ssh_review::review_shell_command(review_config, &payload).await
                {
                    Ok(verdict) => Some(SshAuditReview {
                        allowed: verdict.allowed,
                        user_approved: false,
                        reason: verdict.reason,
                    }),
                    Err(error) => {
                        command_history::record(
                            workspace_id,
                            tool_name,
                            server_id,
                            command,
                            CommandHistoryStatus::Blocked,
                            &format!("审查服务异常：{error}"),
                        );
                        let blocked = SshAuditReview {
                            allowed: false,
                            user_approved: false,
                            reason: format!("审查服务异常：{error}"),
                        };
                        let record_result = manager
                            .record_review_blocked(
                                workspace.to_path_buf(),
                                workspace_id.to_string(),
                                session_title.to_string(),
                                server_id.to_string(),
                                session_id.to_string(),
                                command.to_string(),
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
            Ok(_) => {
                if elevate {
                    let confirm_reason = "该服务器已关闭执行前审查，提权命令需用户确认";
                    let outcome = if crate::agent::rig_ext::review_confirm::needs_user_confirmation(
                        confirm_reason,
                        command,
                        true,
                    ) {
                        Some(
                            crate::agent::rig_ext::review_confirm::request_confirmation(
                                app_handle.as_ref(),
                                cancel_rx.clone(),
                                crate::agent::rig_ext::review_confirm::ConfirmRequest {
                                    workspace_id: workspace_id.to_string(),
                                    tool: tool_name.to_string(),
                                    target: format!("SSH server {server_id}"),
                                    command: command.to_string(),
                                    reason: confirm_reason.to_string(),
                                    elevated: true,
                                },
                            )
                            .await,
                        )
                    } else {
                        None
                    };
                    match outcome {
                        Some(crate::agent::rig_ext::review_confirm::ConfirmOutcome::Approved) => {
                            Some(SshAuditReview {
                                allowed: true,
                                user_approved: true,
                                reason: "服务器已关闭执行前审查，提权（sudo）命令经用户确认放行"
                                    .to_string(),
                            })
                        }
                        Some(crate::agent::rig_ext::review_confirm::ConfirmOutcome::Cancelled) => {
                            return Err(ToolExecutionError::cancelled(
                                "错误：等待提权确认期间本轮已取消，命令未执行。",
                            ));
                        }
                        outcome => {
                            let refusal = match outcome {
                                Some(crate::agent::rig_ext::review_confirm::ConfirmOutcome::Denied) => {
                                    "错误：用户已明确拒绝执行该提权（sudo）命令，不得重试或改写绕过。"
                                }
                                Some(crate::agent::rig_ext::review_confirm::ConfirmOutcome::Timeout) => {
                                    "错误：提权（sudo）命令需要用户确认，但等待超时未获批准，已拒绝执行。"
                                }
                                _ => "错误：提权（sudo）命令命中不可经用户确认放行的灾难模式（无差别删除/覆写类），已直接拒绝执行。",
                            };
                            // 审计 reason 与实际结局对齐：Denied / Timeout / 未弹窗（灾难模式）
                            // 是三种不同的拦截语义，不能共用「未获批准」。
                            let audit_reason = match outcome {
                                Some(
                                    crate::agent::rig_ext::review_confirm::ConfirmOutcome::Denied,
                                ) => "用户已明确拒绝执行提权命令",
                                Some(
                                    crate::agent::rig_ext::review_confirm::ConfirmOutcome::Timeout,
                                ) => "提权命令等待用户确认超时，未获批准",
                                _ => "提权命令命中不可确认放行的灾难模式，未发起用户确认",
                            };
                            command_history::record(
                                workspace_id,
                                tool_name,
                                server_id,
                                command,
                                CommandHistoryStatus::Blocked,
                                audit_reason,
                            );
                            let blocked = SshAuditReview {
                                allowed: false,
                                user_approved: false,
                                reason: audit_reason.to_string(),
                            };
                            if let Err(error) = manager
                                .record_review_blocked(
                                    workspace.to_path_buf(),
                                    workspace_id.to_string(),
                                    session_title.to_string(),
                                    server_id.to_string(),
                                    session_id.to_string(),
                                    command.to_string(),
                                    blocked,
                                )
                                .await
                            {
                                eprintln!("[ssh-command-gate] 提权拦截审计写入失败：{error}");
                            }
                            return Err(ToolExecutionError::refused(refusal));
                        }
                    }
                } else {
                    None
                }
            }
            Err(error) => return Err(ToolExecutionError::other(format!("错误：{error}"))),
        },
    };

    if let Some(review) = &review_outcome {
        if !review.allowed {
            let reason = review.reason.clone();
            let confirm_outcome = if crate::agent::rig_ext::review_confirm::needs_user_confirmation(
                &reason, command, elevate,
            ) {
                Some(
                    crate::agent::rig_ext::review_confirm::request_confirmation(
                        app_handle.as_ref(),
                        cancel_rx.clone(),
                        crate::agent::rig_ext::review_confirm::ConfirmRequest {
                            workspace_id: workspace_id.to_string(),
                            tool: tool_name.to_string(),
                            target: format!("SSH server {server_id}"),
                            command: command.to_string(),
                            reason: reason.clone(),
                            elevated: elevate,
                        },
                    )
                    .await,
                )
            } else {
                None
            };
            match confirm_outcome {
                Some(crate::agent::rig_ext::review_confirm::ConfirmOutcome::Approved) => {
                    review_outcome = Some(SshAuditReview {
                        allowed: true,
                        user_approved: true,
                        reason: format!("安全审查未自动放行，经用户确认放行：{reason}"),
                    });
                }
                Some(crate::agent::rig_ext::review_confirm::ConfirmOutcome::Cancelled) => {
                    return Err(ToolExecutionError::cancelled(
                        "错误：等待用户确认期间本轮已取消，命令未执行。",
                    ));
                }
                Some(crate::agent::rig_ext::review_confirm::ConfirmOutcome::Denied) => {
                    command_history::record(
                        workspace_id,
                        tool_name,
                        server_id,
                        command,
                        CommandHistoryStatus::Blocked,
                        &format!("用户已明确拒绝执行：{reason}"),
                    );
                    if let Err(error) = manager
                        .record_review_blocked(
                            workspace.to_path_buf(),
                            workspace_id.to_string(),
                            session_title.to_string(),
                            server_id.to_string(),
                            session_id.to_string(),
                            command.to_string(),
                            SshAuditReview {
                                allowed: false,
                                user_approved: false,
                                reason: format!("用户已明确拒绝执行：{reason}"),
                            },
                        )
                        .await
                    {
                        eprintln!("[ssh-command-gate] 拒绝拦截审计写入失败：{error}");
                    }
                    return Err(ToolExecutionError::refused(
                        "错误：用户已明确拒绝执行该命令，不得重试或改写绕过。",
                    ));
                }
                Some(crate::agent::rig_ext::review_confirm::ConfirmOutcome::Timeout) => {
                    command_history::record(
                        workspace_id,
                        tool_name,
                        server_id,
                        command,
                        CommandHistoryStatus::Blocked,
                        &reason,
                    );
                    let record_result = manager
                        .record_review_blocked(
                            workspace.to_path_buf(),
                            workspace_id.to_string(),
                            session_title.to_string(),
                            server_id.to_string(),
                            session_id.to_string(),
                            command.to_string(),
                            review.clone(),
                        )
                        .await;
                    match record_result {
                        Ok(record) => {
                            return Err(ToolExecutionError::refused(
                                crate::agent::ssh_review::with_confirm_guidance(
                                    format!(
                                        "错误：命令已被安全审查拦截：{reason}。\n\n{}",
                                        crate::ssh_tool::render_ssh_audit_record_markdown(&record)
                                    ),
                                    &reason,
                                ),
                            ));
                        }
                        Err(error) => {
                            return Err(ToolExecutionError::refused(
                                crate::agent::ssh_review::with_confirm_guidance(
                                    format!("错误：命令已被安全审查拦截：{reason}；写入 SSH 审计失败：{error}。如需放行，可在 SSH 工具配置中关闭该服务器的「执行前审查」开关。"),
                                    &reason,
                                ),
                            ));
                        }
                    }
                }
                // needs_user_confirmation 判定不可确认（含灾难模式）：从未发起弹窗，
                // 不得附带「可经用户确认放行」指引，否则模型会引导用户确认后重试，
                // 而重试仍会被拒绝。
                None => {
                    let blocked_reason = "命令已被安全审查拦截，且不可经用户确认放行";
                    command_history::record(
                        workspace_id,
                        tool_name,
                        server_id,
                        command,
                        CommandHistoryStatus::Blocked,
                        blocked_reason,
                    );
                    let blocked = SshAuditReview {
                        allowed: false,
                        user_approved: false,
                        reason: blocked_reason.to_string(),
                    };
                    let record_result = manager
                        .record_review_blocked(
                            workspace.to_path_buf(),
                            workspace_id.to_string(),
                            session_title.to_string(),
                            server_id.to_string(),
                            session_id.to_string(),
                            command.to_string(),
                            blocked,
                        )
                        .await;
                    let record_markdown = record_result
                        .map(|record| {
                            format!(
                                "\n\n{}",
                                crate::ssh_tool::render_ssh_audit_record_markdown(&record)
                            )
                        })
                        .unwrap_or_default();
                    return Err(ToolExecutionError::refused(format!(
                        "错误：命令已被安全审查拦截，且不可经用户确认放行（{reason}），不得重试或改写绕过。{record_markdown}"
                    )));
                }
            }
        }
    }
    Ok(review_outcome)
}

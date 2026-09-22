use super::common::resolve_path;
use crate::agent::command_history::{self, CommandHistoryStatus};
use crate::agent::tools::{context::ToolContext, registry::AgentTool, ToolResult};
use crate::ssh_tool::sync::{SyncDirectory, SyncResult};
use crate::ssh_tool::{SshAuditRecord, SshAuditReview, SshServerConfig, SshSessionManager};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::Emitter;

pub(super) fn sync_directory_tool(manager: SshSessionManager) -> Box<dyn AgentTool> {
    Box::new(SyncDirectoryTool { manager })
}
struct SyncDirectoryTool {
    manager: SshSessionManager,
}

#[async_trait]
impl AgentTool for SyncDirectoryTool {
    fn name(&self) -> &'static str {
        "sync_directory"
    }
    fn description(&self) -> &'static str {
        "用 rsync over SSH 将本地目录内容同步到已配置服务器的指定绝对目录。ssh_profile 是 ssh_list_servers 返回的 id。复用密码/私钥及主机指纹校验；执行前安全审查。默认不删除远端多余文件，dry_run=true 仅预演（仍需连接服务器）。只上传目录内容，不额外嵌套源目录名；保留时间、权限及安全符号链接，不跟随符号链接读取目录外文件；排除 .git/ 和 .jkcodingagent/。要求本机 rsync 3.1+、OpenSSH，远端 rsync 3.0+，目标父目录须存在。不提权、传输不自动重试；超时使用服务器配置（最大300秒），中止/失败可能已同步部分文件。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object", "additionalProperties":false,
            "properties":{
                "ssh_profile":{"type":"string","minLength":1,"description":"已启用 SSH 服务器的 id，来自 ssh_list_servers"},
                "source":{"type":"string","minLength":1,"description":"本地已有目录，受工作区/额外允许目录权限约束"},
                "destination":{"type":"string","minLength":1,"description":"远端绝对 POSIX 目录，不能为 / 或包含 .、.."},
                "delete":{"type":"boolean","default":false,"description":"删除远端目标目录内源端不存在的文件；排除目录不删除"},
                "dry_run":{"type":"boolean","default":false,"description":"仅计算和报告变化，不写入或删除远端文件"}
            },
            "required":["ssh_profile","source","destination"]
        })
    }
    async fn execute(&self, args: &Value, context: &ToolContext) -> ToolResult {
        match self.execute_inner(args, context).await {
            Ok(result) => result,
            Err(error) if context.cancel_rx.as_ref().is_some_and(|rx| *rx.borrow()) => {
                ToolResult::cancelled(error)
            }
            Err(error) => ToolResult::recoverable_error(error),
        }
    }
}

impl SyncDirectoryTool {
    async fn execute_inner(
        &self,
        args: &Value,
        context: &ToolContext,
    ) -> Result<ToolResult, String> {
        let mut request: SyncDirectory =
            serde_json::from_value(args.clone()).map_err(|e| format!("同步参数无效：{e}"))?;
        request.validate()?;
        let ctx = context.clone();
        let source = request.source.clone();
        let source = tokio::task::spawn_blocking(move || {
            let path = resolve_path(&ctx, &source)?
                .canonicalize()
                .map_err(|e| format!("解析源目录失败：{e}"))?;
            // 即便关闭工作区限制，解析符号链接后也要再次检查敏感路径。
            resolve_path(&ctx, path.to_str().ok_or("源目录必须为 UTF-8 路径")?)?;
            if !path.is_dir() {
                return Err("source 必须是已有目录".to_string());
            }
            // 会话沙箱及 local_zsh 产物本就位于 .jkcodingagent 下，
            // 不能仅凭祖先目录名拒绝；敏感子路径由 resolve_path 校验。
            if path.components().any(|c| c.as_os_str() == ".git")
                || path.file_name().is_some_and(|name| name == ".jkcodingagent")
                || (path.file_name().is_some_and(|name| name == "local_env")
                    && path.parent().and_then(|p| p.file_name()).is_some_and(|name| name == ".jkcodingagent"))
            {
                return Err(format!(
                    "不能同步应用配置根目录或 Git 元数据目录：{}。当前工作区：{}。请指定工作区或 local_zsh 下具体的产物子目录。",
                    path.display(), ctx.workspace.display()
                ));
            }
            Ok::<_, String>(path)
        })
        .await
        .map_err(|e| e.to_string())??;
        request.source = source.to_str().ok_or("源目录必须为 UTF-8 路径")?.into();
        let server = self
            .manager
            .server_config_async(request.ssh_profile.clone())
            .await?;
        let command = request.command_description();
        let review = review(context, args, &server, &command).await;
        if !review.allowed {
            command_history::record(
                &context.workspace_id,
                "sync_directory",
                &server.id,
                &command,
                CommandHistoryStatus::Blocked,
                &review.reason,
            );
            self.manager
                .record_review_blocked(
                    context.workspace.clone(),
                    context.workspace_id.clone(),
                    context.session_title.clone(),
                    server.id,
                    context.workspace_id.clone(),
                    command,
                    review.clone(),
                )
                .await?;
            return Err(crate::agent::ssh_review::with_confirm_guidance(
                format!("目录同步已被安全审查拦截：{}", review.reason),
                &review.reason,
            ));
        }
        let app = context.app_handle.clone();
        let workspace_id = context.workspace_id.clone();
        let tool_call_id = context.current_tool_call_id.clone();
        let profile = server.id.clone();
        let progress = Arc::new(move |p: crate::ssh_tool::sync::SyncProgress| {
            if let Some(app) = &app {
                if let Err(error) = app.emit(
                    "ssh-sync-progress",
                    json!({
                        "workspaceId":workspace_id, "toolCallId":tool_call_id,
                        "sshProfile":profile, "progress":p
                    }),
                ) {
                    eprintln!("[sync_directory] 发送进度事件失败：{error}");
                }
            }
        });
        let result = self
            .manager
            .sync_directory(
                server,
                request.clone(),
                source,
                context.cancel_rx.clone(),
                progress,
            )
            .await;
        let record = audit_record(context, &request, command.clone(), review, &result);
        let audit_error = self.manager.append_sync_audit(record).await.err();
        let mut result = match result {
            Ok(result) => result,
            Err(error) => {
                command_history::record(
                    &context.workspace_id,
                    "sync_directory",
                    &request.ssh_profile,
                    &command,
                    CommandHistoryStatus::Executed,
                    &error,
                );
                return Err(match audit_error {
                    Some(audit) => format!("{error}；写入 SSH 审计失败：{audit}"),
                    None => error,
                });
            }
        };
        result.audit_error = audit_error;
        let text = serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?;
        command_history::record(
            &context.workspace_id,
            "sync_directory",
            &request.ssh_profile,
            &command,
            CommandHistoryStatus::Executed,
            &text,
        );
        let mut tool_result = if result.cancelled {
            ToolResult::cancelled(format!("目录同步已取消，远端可能已有部分变更。\n{text}"))
        } else if result.timed_out || result.exit_code != Some(0) {
            ToolResult::recoverable_error(format!(
                "目录同步失败，远端可能已有部分变更。请检查 rsync/OpenSSH 版本及返回信息。\n{text}"
            ))
        } else {
            ToolResult::success_data(json!(result), text.clone(), text)
        };
        tool_result.data = Some(json!(result));
        Ok(tool_result)
    }
}

async fn review(
    context: &ToolContext,
    args: &Value,
    server: &SshServerConfig,
    command: &str,
) -> SshAuditReview {
    let Some(config) = &context.ssh_review else {
        return SshAuditReview {
            allowed: false,
            reason: "未配置安全审查模型，已拒绝目录同步".into(),
        };
    };
    if !server.review_enabled {
        return SshAuditReview {
            allowed: true,
            reason: "服务器配置显式关闭执行前审查".into(),
        };
    }
    use crate::agent::ssh_review::{
        review_shell_command, CommandReviewTarget, SshReviewServerInfo,
    };
    let payload = crate::agent::tools::review_context::build_review_payload(
        context,
        Some(args),
        CommandReviewTarget::Ssh(SshReviewServerInfo {
            id: server.id.clone(),
            description: server.description.clone(),
            host: server.host.clone(),
            port: server.port,
            username: server.username.clone(),
            tags: server.tags.clone(),
        }),
        command.into(),
        None,
    );
    match review_shell_command(config, &payload).await {
        Ok(verdict) => SshAuditReview {
            allowed: verdict.allowed,
            reason: verdict.reason,
        },
        Err(error) => SshAuditReview {
            allowed: false,
            reason: format!("安全审查服务失败：{error}"),
        },
    }
}

fn audit_record(
    context: &ToolContext,
    request: &SyncDirectory,
    command: String,
    review: SshAuditReview,
    result: &Result<SyncResult, String>,
) -> SshAuditRecord {
    let output = result.as_ref().ok();
    SshAuditRecord {
        created_at: chrono::Utc::now().to_rfc3339(),
        workspace_path: context.workspace.to_string_lossy().into_owned(),
        workspace_id: context.workspace_id.clone(),
        session_title: context.session_title.clone(),
        server_id: request.ssh_profile.clone(),
        session_id: context.workspace_id.clone(),
        command,
        exit_code: output.and_then(|r| r.exit_code),
        stdout: output
            .map(|r| r.stdout.chars().take(8000).collect())
            .unwrap_or_default(),
        stderr: output
            .map(|r| r.stderr.chars().take(8000).collect())
            .unwrap_or_default(),
        duration_ms: output.map(|r| r.duration_ms as u128),
        truncated: output.is_some_and(|r| {
            r.truncated || r.stdout.chars().count() > 8000 || r.stderr.chars().count() > 8000
        }),
        interactive_blocked: false,
        error: result.as_ref().err().cloned().or_else(|| {
            output.and_then(|r| {
                if r.cancelled {
                    Some("同步已取消".into())
                } else if r.timed_out {
                    Some("同步超时".into())
                } else if r.exit_code != Some(0) {
                    Some(format!("rsync 非零退出：{:?}", r.exit_code))
                } else {
                    None
                }
            })
        }),
        review: Some(review),
    }
}

#[cfg(test)]
#[path = "sync_directory_tests.rs"]
mod tests;

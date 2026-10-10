use rig::tool::ToolExecutionError;
use serde_json::Value;

use super::{session_title_of, TermToolCtx};
use crate::agent::command_history::{self, CommandHistoryStatus};
use crate::ssh_tool::SshAuditReview;

// ---------------------------------------------------------------------------
// 审查门禁（send / open-command 共用；口径对齐 ssh_exec）
// ---------------------------------------------------------------------------

/// 命令审查门禁：未配置审查模型 fail-closed；服务器开启审查则送审（拒绝时
/// 记台账 + 审计并返回 refused，不弹窗——人工放行属 M3 方案 C）；服务器显式
/// 关闭审查按配置放行（返回 None）。
pub(super) async fn review_term_command(
    ctx: &TermToolCtx,
    tool_name: &str,
    server_id: &str,
    session_id: &str,
    command: &str,
    args: Option<&Value>,
    // 终端现场（仅 send 路径：让审查模型看到真实屏幕而非裸按键碎片）。
    screen_context: Option<String>,
) -> Result<Option<SshAuditReview>, ToolExecutionError> {
    // 会话标题只有拒绝路径（拦截审计）需要；放行路径不做这次 DB 读。
    let refuse =
        |reason: String| blocked_refusal(ctx, tool_name, server_id, session_id, command, reason);

    let Some(review_config) = ctx.review_context.config.clone() else {
        return Err(refuse("未配置安全审查，无法评估输入安全性".to_string()).await);
    };
    let server = ctx
        .manager
        .server_config_async(server_id.to_string())
        .await
        .map_err(|error| ToolExecutionError::other(format!("错误：{error}")))?;
    if !server.review_enabled {
        // 服务器显式关闭「执行前审查」：设计内的豁免通道，放行但落审计。
        return Ok(None);
    }
    let mut payload = ctx.review_context.build_payload(
        &ctx.workspace_id,
        args,
        crate::agent::ssh_review::CommandReviewTarget::Ssh(
            crate::agent::ssh_review::SshReviewServerInfo {
                id: server.id.clone(),
                description: server.description.clone(),
                host: server.host.clone(),
                port: server.port,
                username: server.username.clone(),
                tags: server.tags.clone(),
                elevated: false,
            },
        ),
        command.to_string(),
        None,
    );
    payload.screen_context = screen_context;
    match crate::agent::ssh_review::review_shell_command(&review_config, &payload).await {
        Ok(verdict) => {
            let review = SshAuditReview {
                allowed: verdict.allowed,
                reason: verdict.reason,
            };
            if !review.allowed {
                let reason = review.reason.clone();
                return Err(refuse(reason).await);
            }
            Ok(Some(review))
        }
        Err(error) => Err(refuse(format!("审查服务异常：{error}")).await),
    }
}

/// 审查拒绝的收口：命令台账记 Blocked + 拦截审计 + refused 错误（不弹窗，
/// 人工放行属 M3 方案 C）。会话标题在此按需读取（放行路径不消耗这次 DB 读）。
#[allow(clippy::too_many_arguments)]
async fn blocked_refusal(
    ctx: &TermToolCtx,
    tool_name: &str,
    server_id: &str,
    session_id: &str,
    command: &str,
    reason: String,
) -> ToolExecutionError {
    let session_title = session_title_of(ctx).await;
    command_history::record(
        &ctx.workspace_id,
        tool_name,
        server_id,
        command,
        CommandHistoryStatus::Blocked,
        &reason,
    );
    let review = SshAuditReview {
        allowed: false,
        reason: reason.clone(),
    };
    if let Ok(record) = ctx
        .manager
        .record_review_blocked(
            ctx.workspace.clone(),
            ctx.workspace_id.clone(),
            session_title,
            server_id.to_string(),
            session_id.to_string(),
            command.to_string(),
            review,
        )
        .await
    {
        ToolExecutionError::refused(format!(
            "错误：命令已被安全审查拦截：{reason}。\n\n{}",
            crate::ssh_tool::render_ssh_audit_record_markdown(&record)
        ))
    } else {
        ToolExecutionError::refused(format!("错误：命令已被安全审查拦截：{reason}。"))
    }
}

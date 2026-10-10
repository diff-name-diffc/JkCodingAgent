//! SSH 交互终端工具组：`ssh_term_open` / `ssh_term_send` / `ssh_term_read` /
//! `ssh_term_close` / `ssh_term_list`。
//!
//! 「tmux 优先、avt 兜底」：open 默认探测远端 tmux 并 attach-or-create（模板
//! 命令免 LLM 审查，见设计文档 §7），无 tmux 回退 russh PTY + avt 屏幕模型。
//! send 的审查口径与 ssh_exec 一致：未配置审查模型 fail-closed；服务器开启
//! 审查则逐次送审（text + intent）；服务器显式关闭审查按配置放行（设计内
//! 豁免通道）。审查拒绝直接 refused 不弹窗——人工放行是 M3 方案 C 的范围。

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::agent::db::DispatcherDb;
use crate::agent::rig_ext::review::RigReviewContext;
use crate::agent::rig_ext::tools::common::{string_arg, usize_arg};
use crate::agent::rig_ext::tools::deps::RigToolDeps;
use crate::ssh_tool::term::TermSessionRegistry;
use crate::ssh_tool::{SshAuditReview, SshSessionManager};
use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};

mod input;
mod open;
mod read;
mod review;
mod send;

use open::ssh_term_open_tool;
use read::ssh_term_read_tool;
use send::ssh_term_send_tool;

#[derive(Clone)]
struct TermToolCtx {
    registry: TermSessionRegistry,
    manager: SshSessionManager,
    workspace: PathBuf,
    workspace_id: String,
    db: DispatcherDb,
    review_context: RigReviewContext,
}

pub(super) fn ssh_term_tools(deps: &RigToolDeps) -> Vec<PortableDynamicTool> {
    let ctx = TermToolCtx {
        registry: deps.term_registry.clone(),
        manager: deps.ssh_manager.clone(),
        workspace: deps.workspace.clone(),
        workspace_id: deps.workspace_id.clone(),
        db: deps.db.clone(),
        review_context: deps.review.clone(),
    };
    vec![
        ssh_term_open_tool(ctx.clone()),
        ssh_term_send_tool(ctx.clone()),
        ssh_term_read_tool(ctx.clone()),
        ssh_term_resize_tool(ctx.clone()),
        ssh_term_close_tool(ctx.clone()),
        ssh_term_list_tool(ctx),
    ]
}

/// 取消信号在本层（agent 循环 task-local 作用域边界）读取一次后向下传递。
fn current_cancel_rx() -> Option<tokio::sync::watch::Receiver<bool>> {
    crate::agent::rig_ext::r#loop::invocation::ToolInvocationContext::current()
        .map(|context| context.cancel_rx)
}

fn map_term_error(error: crate::ssh_tool::term::TermError) -> ToolExecutionError {
    use crate::ssh_tool::term::TermError;
    match error {
        TermError::NotFound(message) => ToolExecutionError::not_found(format!("错误：{message}")),
        TermError::QuotaExceeded(message) => ToolExecutionError::other(message),
        TermError::Exited(message) => {
            ToolExecutionError::other(format!("错误：{message}")).with_code("command_failed")
        }
        TermError::ExternalStateUnknown(message) => {
            ToolExecutionError::other(format!("错误：{message}"))
                .with_code("external_state_unknown")
        }
        TermError::Open(message) | TermError::Read(message) | TermError::Write(message) => {
            ToolExecutionError::other(format!("错误：{message}"))
        }
    }
}

fn render_json<T: serde::Serialize>(payload: &T) -> Result<String, ToolExecutionError> {
    serde_json::to_string_pretty(payload)
        .map_err(|error| ToolExecutionError::other(format!("错误：序列化结果失败：{error}")))
}

async fn session_title_of(ctx: &TermToolCtx) -> String {
    ctx.db
        .get_session_title_async(&ctx.workspace_id)
        .await
        .unwrap_or_else(|error| {
            eprintln!("[ssh-term] 读取会话标题失败，审计记录标题留空：{error}");
            String::new()
        })
}

/// ssh_term 组的活动审计（open/send/close 成功路径；拦截走 record_review_blocked）。
#[allow(clippy::too_many_arguments)]
async fn append_term_audit(
    ctx: &TermToolCtx,
    server_id: &str,
    session_id: &str,
    command: String,
    review: Option<SshAuditReview>,
) {
    let session_title = session_title_of(ctx).await;
    if let Err(error) = ctx
        .manager
        .append_term_activity_audit(
            ctx.workspace.clone(),
            ctx.workspace_id.clone(),
            session_title,
            server_id.to_string(),
            session_id.to_string(),
            command,
            review,
        )
        .await
    {
        eprintln!("[ssh-term] 写入终端活动审计失败：{error}");
    }
}

// ---------------------------------------------------------------------------
// ssh_term_read / close / list
// ---------------------------------------------------------------------------

fn ssh_term_resize_tool(ctx: TermToolCtx) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "ssh_term_resize",
        "调整终端尺寸（同时同步远端 PTY 的 window_change 与本地屏幕模型）。全屏程序显示错乱或需要更宽的输出布局时使用；cols 夹紧 40..200、rows 夹紧 10..60。调整后用 ssh_term_read 读取新布局下的屏幕。",
        json!({
            "type": "object",
            "properties": {
                "term_id": { "type": "string", "description": "ssh_term_open 返回的 term_id" },
                "cols": { "type": "integer", "description": "目标列数（夹紧 40..200）", "minimum": 40, "maximum": 200 },
                "rows": { "type": "integer", "description": "目标行数（夹紧 10..60）", "minimum": 10, "maximum": 60 }
            },
            "required": ["term_id", "cols", "rows"]
        }),
        move |args| {
            let ctx = ctx.clone();
            Box::pin(async move { ssh_term_resize_text(&args, ctx).await.map(ToolOutput::text) })
        },
    )
}

async fn ssh_term_resize_text(
    args: &Value,
    ctx: TermToolCtx,
) -> Result<String, ToolExecutionError> {
    let Some(term_id) = string_arg(args, "term_id") else {
        return Err(ToolExecutionError::invalid_args(
            "错误：缺少必填参数 term_id。".to_string(),
        ));
    };
    let (Some(cols), Some(rows)) = (usize_arg(args, "cols"), usize_arg(args, "rows")) else {
        return Err(ToolExecutionError::invalid_args(
            "错误：缺少必填参数 cols / rows。".to_string(),
        ));
    };
    ctx.registry
        .resize(&term_id, cols, rows)
        .await
        .map_err(map_term_error)?;
    Ok(json!({ "termId": term_id, "ok": true, "cols": cols, "rows": rows }).to_string())
}

fn ssh_term_close_tool(ctx: TermToolCtx) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "ssh_term_close",
        "关闭一个终端会话；已移除的 term_id 返回 not_found。tmux 会话默认 detach：远端进程仍在运行时现场保留，之后传同名 tmuxSession 重新 open 可恢复；killTmuxSession=true 时彻底终止远端 tmux 会话与其中进程。裸 PTY 关闭后无法保留或恢复交互现场。用完的终端应及时关闭以释放配额。",
        json!({
            "type": "object",
            "properties": {
                "term_id": { "type": "string", "description": "ssh_term_open 返回的 term_id" },
                "killTmuxSession": {
                    "type": "boolean",
                    "description": "默认 false（detach 保留现场）；true 时执行 tmux kill-session 彻底回收"
                }
            },
            "required": ["term_id"]
        }),
        move |args| {
            let ctx = ctx.clone();
            Box::pin(async move { ssh_term_close_text(&args, ctx).await.map(ToolOutput::text) })
        },
    )
}

async fn ssh_term_close_text(args: &Value, ctx: TermToolCtx) -> Result<String, ToolExecutionError> {
    let Some(term_id) = string_arg(args, "term_id") else {
        return Err(ToolExecutionError::invalid_args(
            "错误：缺少必填参数 term_id。".to_string(),
        ));
    };
    let kill = input::parse_strict_bool(args, "killTmuxSession")?;
    // close 成功即从注册表移除；审计字段（server/session）必须先行取好，
    // 事后反查只会拿不到，session_id 将失真为 term_id。
    let target = ctx
        .registry
        .list(None)
        .into_iter()
        .find(|info| info.term_id == term_id)
        .map(|info| (info.server_id, info.session_id));
    let payload = ctx
        .registry
        .close(&term_id, kill)
        .await
        .map_err(map_term_error)?;
    if let Some((server_id, session_id)) = target {
        let action = match (&payload.tmux_session, kill) {
            (Some(name), true) => format!("ssh_term_close tmux kill-session -t {name}"),
            (Some(name), false) => format!("ssh_term_close detach (tmux: {name})"),
            (None, _) => "ssh_term_close".to_string(),
        };
        append_term_audit(&ctx, &server_id, &session_id, action, None).await;
    }
    render_json(&payload)
}

fn ssh_term_list_tool(ctx: TermToolCtx) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "ssh_term_list",
        "列出当前打开的交互终端会话（term_id、服务器、tmux_session、活动时间、退出状态）。长任务或上下文重载后用它重新发现自己打开的终端；tmuxSession 可用于断连后传同名恢复现场。",
        json!({
            "type": "object",
            "properties": {
                "server_id": { "type": "string", "description": "可选，按服务器过滤" }
            },
            "required": []
        }),
        move |args| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let server_id = string_arg(&args, "server_id");
                let terms = ctx.registry.list(server_id.as_deref());
                match serde_json::to_string_pretty(&json!({ "terms": terms })) {
                    Ok(text) => Ok(ToolOutput::text(text)),
                    Err(error) => Err(ToolExecutionError::other(format!(
                        "错误：序列化终端列表失败：{error}"
                    ))),
                }
            })
        },
    )
}

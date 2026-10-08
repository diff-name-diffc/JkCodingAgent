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

use super::super::common::{string_arg, usize_arg, with_compression_parameters};
use crate::agent::command_history::{self, CommandHistoryStatus};
use crate::agent::db::DispatcherDb;
use crate::agent::rig_ext::review::RigReviewContext;
use crate::agent::rig_ext::tools::deps::RigToolDeps;
use crate::ssh_tool::term::{
    TermOpenParams, TermSessionRegistry, TmuxPreference, DEFAULT_COLS, DEFAULT_ROWS,
};
use crate::ssh_tool::{SshAuditReview, SshSessionManager};
use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};

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
        TermError::Open(message) | TermError::Write(message) => {
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
// ssh_term_open
// ---------------------------------------------------------------------------

fn ssh_term_open_tool(ctx: TermToolCtx) -> PortableDynamicTool {
    let parameters = with_compression_parameters(
        json!({
            "type": "object",
            "properties": {
                "server_id": { "type": "string", "description": "ssh_list_servers 返回的服务器 id" },
                "session_id": {
                    "type": "string",
                    "description": "聊天会话 id。同一 server_id + session_id 复用 SSH 连接"
                },
                "command": {
                    "type": "string",
                    "description": "可选。直接以交互模式启动的命令（如 python、top）。提供时跳过 tmux、直接 PTY+exec；该命令会经过安全审查"
                },
                "tmux": {
                    "type": "string",
                    "enum": ["auto", "off"],
                    "description": "tmux 叠加策略，默认 auto：探测到远端有 tmux 则以 attach-or-create 进入 tmux 会话（断连后现场保留、同名可恢复、人类可 tmux attach 共屏）；off 强制裸 shell"
                },
                "tmuxSession": {
                    "type": "string",
                    "description": "可选。自定义 tmux 会话名，必须以 jkagent- 前缀开头（如 jkagent-install-nginx）。跨连接恢复现场时传回上次返回的同名值"
                },
                "cols": { "type": "integer", "description": "终端列数，默认 80（夹紧 40..200）", "minimum": 40, "maximum": 200 },
                "rows": { "type": "integer", "description": "终端行数，默认 24（夹紧 10..60）", "minimum": 10, "maximum": 60 }
            },
            "required": ["server_id", "session_id"]
        }),
        false,
        crate::agent::rig_ext::tools::spec::DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
        "返回含首屏快照（screen），通常一至两屏以内；compress=true 适合 tmux 全屏 UI 等大屏场景。",
    );
    PortableDynamicTool::new(
        "ssh_term_open",
        "打开一个交互式远程终端会话（真实 PTY），返回 termId 与首屏。默认 tmux=auto：远端有 tmux 则进入 attach-or-create 的 tmux 会话（现场由 tmux daemon 保活，断连后传同名 tmuxSession 重开即恢复；人类也可 ssh 登录后 tmux attach 围观），无 tmux 则回退自建虚拟终端。配额：每服务器 ≤4、全局 ≤16。适用于 ssh_exec 无法处理的交互场景（REPL、全屏程序、分步向导、确认提示）；能用非交互命令完成的事优先用 ssh_exec。不要用本工具输入任何密码/密钥——遇到密码提示应改用无交互路径或请用户介入。",
        parameters,
        move |args| {
            let ctx = ctx.clone();
            Box::pin(async move { ssh_term_open_text(&args, ctx).await.map(ToolOutput::text) })
        },
    )
}

async fn ssh_term_open_text(args: &Value, ctx: TermToolCtx) -> Result<String, ToolExecutionError> {
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
    let command = string_arg(args, "command");
    let tmux_off = string_arg(args, "tmux").as_deref() == Some("off");
    let tmux_session = string_arg(args, "tmuxSession");
    let cols = usize_arg(args, "cols").unwrap_or(DEFAULT_COLS);
    let rows = usize_arg(args, "rows").unwrap_or(DEFAULT_ROWS);

    // 带 command 的 open 等价命令执行：走完整审查门禁（§4.1）。
    let review = match &command {
        Some(command) => {
            review_term_command(
                &ctx,
                "ssh_term_open",
                &server_id,
                &session_id,
                command,
                Some(args),
                // open(command) 路径终端尚未开，无现场。
                None,
            )
            .await?
        }
        None => None,
    };

    let payload = ctx
        .registry
        .open(
            &ctx.manager,
            TermOpenParams {
                server_id: server_id.clone(),
                session_id: session_id.clone(),
                cols,
                rows,
                command,
                tmux: if tmux_off {
                    TmuxPreference::Off
                } else {
                    TmuxPreference::Auto
                },
                tmux_session,
            },
        )
        .await
        .map_err(map_term_error)?;

    let audit_command = match (&payload.tmux_session, &payload.note) {
        (Some(name), _) => format!("ssh_term_open tmux new -A -s {name} ({cols}x{rows})"),
        (None, _) => format!("ssh_term_open shell ({cols}x{rows})"),
    };
    // command 路径的审计命令串用真实命令；裸/tmux 模板路径用形态描述。
    let audit_command = if let Some(command) = args.get("command").and_then(Value::as_str) {
        format!("ssh_term_open exec: {command}")
    } else {
        audit_command
    };
    append_term_audit(&ctx, &server_id, &session_id, audit_command, review).await;
    command_history::record(
        &ctx.workspace_id,
        "ssh_term_open",
        &server_id,
        &format!("open terminal (tmux={:?})", payload.tmux_session),
        CommandHistoryStatus::Executed,
        "打开交互终端",
    );
    render_json(&payload)
}

// ---------------------------------------------------------------------------
// ssh_term_send
// ---------------------------------------------------------------------------

fn ssh_term_send_tool(ctx: TermToolCtx) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "ssh_term_send",
        "向 ssh_term_open 打开的终端发送按键/文本。每次发送都经过安全审查：必填 intent（≤200 字符，说明这次输入的目的）。支持控制字符：\\u0003=Ctrl-C、\\u0004=Ctrl-D、\\u001b[A/B/C/D=方向键、\\r=回车、\\u0002d=tmux detach（Ctrl-B d）。禁止发送任何密码/密钥/令牌——遇到密码提示应改用非交互路径（ssh_exec 的 sudo 参数）或请用户介入。上限 8192 字符；交互密集时尽量合并为一次多行发送以减少审查往返。目标终端已退出时报错并附退出码。",
        json!({
            "type": "object",
            "properties": {
                "term_id": { "type": "string", "description": "ssh_term_open 返回的 termId" },
                "text": { "type": "string", "description": "要发送的按键/文本（支持控制字符转义）" },
                "intent": {
                    "type": "string",
                    "description": "本次输入的目的说明（≤200 字符），随安全审查与审计记录",
                    "maxLength": 200
                }
            },
            "required": ["term_id", "text", "intent"]
        }),
        move |args| {
            let ctx = ctx.clone();
            Box::pin(async move { ssh_term_send_text(&args, ctx).await.map(ToolOutput::text) })
        },
    )
}

async fn ssh_term_send_text(args: &Value, ctx: TermToolCtx) -> Result<String, ToolExecutionError> {
    let Some(term_id) = string_arg(args, "term_id") else {
        return Err(ToolExecutionError::invalid_args(
            "错误：缺少必填参数 term_id。".to_string(),
        ));
    };
    let Some(text) = string_arg(args, "text") else {
        return Err(ToolExecutionError::invalid_args(
            "错误：缺少必填参数 text。".to_string(),
        ));
    };
    let Some(intent) = string_arg(args, "intent") else {
        return Err(ToolExecutionError::invalid_args(
            "错误：缺少必填参数 intent（审查与审计依赖它判断按键目的）。".to_string(),
        ));
    };
    if intent.len() > 200 {
        return Err(ToolExecutionError::invalid_args(
            "错误：intent 不能超过 200 字符。".to_string(),
        ));
    }
    let Some(server_id) = ctx.registry.server_of(&term_id) else {
        return Err(map_term_error(crate::ssh_tool::term::TermError::NotFound(
            format!("终端会话 {term_id} 不存在，可先 ssh_term_list 查看"),
        )));
    };
    let Some(session_id) = ctx
        .registry
        .list(None)
        .into_iter()
        .find(|info| info.term_id == term_id)
        .map(|info| info.session_id)
    else {
        return Err(ToolExecutionError::not_found(format!(
            "错误：终端会话 {term_id} 不存在。"
        )));
    };

    // P3：送审附终端现场——审查模型看到真实屏幕（确认提示/REPL 状态）而非裸按键。
    let screen_context = ctx.registry.screen_context_of(&term_id);
    let review = review_term_command(
        &ctx,
        "ssh_term_send",
        &server_id,
        &session_id,
        &text,
        Some(args),
        screen_context,
    )
    .await?;

    ctx.registry
        .send(&term_id, &text)
        .await
        .map_err(map_term_error)?;
    append_term_audit(
        &ctx,
        &server_id,
        &session_id,
        format!(
            "ssh_term_send: {}（intent: {intent}）",
            summarize_text(&text)
        ),
        review,
    )
    .await;
    command_history::record(
        &ctx.workspace_id,
        "ssh_term_send",
        &server_id,
        &summarize_text(&text),
        CommandHistoryStatus::Executed,
        &intent,
    );
    Ok(
        json!({ "termId": term_id, "ok": true, "hint": "回显与输出经 ssh_term_read 读取" })
            .to_string(),
    )
}

/// 控制字符可视化 + 头尾截断，用于审计与台账展示。
fn summarize_text(text: &str) -> String {
    let visible: String = text
        .chars()
        .map(|c| match c {
            '\r' => "\\r".to_string(),
            '\n' => "\\n".to_string(),
            '\u{3}' => "^C".to_string(),
            '\u{4}' => "^D".to_string(),
            '\u{2}' => "^B".to_string(),
            '\u{1b}' => "ESC".to_string(),
            c if (c as u32) < 32 => format!("^{}", (b'@' + c as u8) as char),
            c => c.to_string(),
        })
        .collect();
    if visible.chars().count() > 120 {
        let head: String = visible.chars().take(100).collect();
        let tail: String = visible.chars().skip(visible.chars().count() - 15).collect();
        format!("{head}…{tail}")
    } else {
        visible
    }
}

// ---------------------------------------------------------------------------
// ssh_term_read / close / list
// ---------------------------------------------------------------------------

fn ssh_term_read_tool(ctx: TermToolCtx) -> PortableDynamicTool {
    let parameters = with_compression_parameters(
        json!({
            "type": "object",
            "properties": {
                "term_id": { "type": "string", "description": "ssh_term_open 返回的 termId" },
                "wait_ms": {
                    "type": "integer",
                    "description": "无新数据时的挂起等待毫秒数（0..25000）。建议 3000..10000：任一输出到达立即返回，避免轮询空转",
                    "minimum": 0,
                    "maximum": 25000
                }
            },
            "required": ["term_id"]
        }),
        false,
        crate::agent::rig_ext::tools::spec::DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
        "返回双轨载荷：newLines（增量行）+ screen（全屏快照）。长时间观察大屏输出时可设 compress=true 只保留关键信息。",
    );
    PortableDynamicTool::new(
        "ssh_term_read",
        "读取终端状态：自上次 read 以来的新增行（newLines）+ 当前可见屏幕（screen，含全屏程序）+ 光标/退出状态。wait_ms 让读取在有新数据前挂起（防轮询）。idleMs 表示距上一帧输出的毫秒数，结合提示符可判断「正在等待输入」。exited=true 时 screen 保留最终画面；tmux 会话断连/detach 的 note 附恢复指引。",
        parameters,
        move |args| {
            let ctx = ctx.clone();
            Box::pin(async move { ssh_term_read_text(&args, ctx).await.map(ToolOutput::text) })
        },
    )
}

async fn ssh_term_read_text(args: &Value, ctx: TermToolCtx) -> Result<String, ToolExecutionError> {
    let Some(term_id) = string_arg(args, "term_id") else {
        return Err(ToolExecutionError::invalid_args(
            "错误：缺少必填参数 term_id。".to_string(),
        ));
    };
    let wait_ms = usize_arg(args, "wait_ms").unwrap_or(0).min(25_000) as u64;
    let payload = ctx
        .registry
        .read(&term_id, wait_ms, current_cancel_rx())
        .await
        .map_err(map_term_error)?;
    render_json(&payload)
}

fn ssh_term_resize_tool(ctx: TermToolCtx) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "ssh_term_resize",
        "调整终端尺寸（同时同步远端 PTY 的 window_change 与本地屏幕模型）。全屏程序显示错乱或需要更宽的输出布局时使用；cols 夹紧 40..200、rows 夹紧 10..60。调整后用 ssh_term_read 读取新布局下的屏幕。",
        json!({
            "type": "object",
            "properties": {
                "term_id": { "type": "string", "description": "ssh_term_open 返回的 termId" },
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
        "关闭一个终端会话（幂等）。tmux 会话默认 detach：远端现场保留，之后传同名 tmuxSession 重新 open 可恢复；killTmuxSession=true 时彻底终止远端 tmux 会话与其中进程。用完的终端应及时关闭以释放配额。",
        json!({
            "type": "object",
            "properties": {
                "term_id": { "type": "string", "description": "ssh_term_open 返回的 termId" },
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
    let kill = args
        .get("killTmuxSession")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let server_id = ctx.registry.server_of(&term_id);
    let payload = ctx
        .registry
        .close(&term_id, kill)
        .await
        .map_err(map_term_error)?;
    if let Some(server_id) = server_id {
        let action = match (&payload.tmux_session, kill) {
            (Some(name), true) => format!("ssh_term_close tmux kill-session -t {name}"),
            (Some(name), false) => format!("ssh_term_close detach (tmux: {name})"),
            (None, _) => "ssh_term_close".to_string(),
        };
        let session_id = ctx
            .registry
            .list(None)
            .into_iter()
            .find(|info| info.term_id == term_id)
            .map(|info| info.session_id);
        // close 已从注册表移除，session_id 拿不到时以 term_id 兜底记录。
        append_term_audit(
            &ctx,
            &server_id,
            session_id.as_deref().unwrap_or(&term_id),
            action,
            None,
        )
        .await;
    }
    render_json(&payload)
}

fn ssh_term_list_tool(ctx: TermToolCtx) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "ssh_term_list",
        "列出当前打开的交互终端会话（termId、服务器、tmux 会话名、活动时间、退出状态）。长任务或上下文重载后用它重新发现自己打开的终端；tmuxSession 可用于断连后传同名恢复现场。",
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

// ---------------------------------------------------------------------------
// 审查门禁（send / open-command 共用；口径对齐 ssh_exec）
// ---------------------------------------------------------------------------

/// 命令审查门禁：未配置审查模型 fail-closed；服务器开启审查则送审（拒绝时
/// 记台账 + 审计并返回 refused，不弹窗——人工放行属 M3 方案 C）；服务器显式
/// 关闭审查按配置放行（返回 None）。
async fn review_term_command(
    ctx: &TermToolCtx,
    tool_name: &str,
    server_id: &str,
    session_id: &str,
    command: &str,
    args: Option<&Value>,
    // 终端现场（仅 send 路径：让审查模型看到真实屏幕而非裸按键碎片）。
    screen_context: Option<String>,
) -> Result<Option<SshAuditReview>, ToolExecutionError> {
    let session_title = session_title_of(ctx).await;
    let refuse = |reason: String| {
        blocked_refusal(
            ctx,
            tool_name,
            server_id,
            session_id,
            command,
            &session_title,
            reason,
        )
    };

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
/// 人工放行属 M3 方案 C）。
#[allow(clippy::too_many_arguments)]
async fn blocked_refusal(
    ctx: &TermToolCtx,
    tool_name: &str,
    server_id: &str,
    session_id: &str,
    command: &str,
    session_title: &str,
    reason: String,
) -> ToolExecutionError {
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
            session_title.to_string(),
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

use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};

use super::review::review_term_command;
use super::{append_term_audit, map_term_error, render_json, TermToolCtx};
use crate::agent::command_history::{self, CommandHistoryStatus};
use crate::agent::rig_ext::tools::common::{string_arg, usize_arg, with_compression_parameters};
use crate::ssh_tool::term::{TermOpenParams, TmuxPreference, DEFAULT_COLS, DEFAULT_ROWS};

// ---------------------------------------------------------------------------
// ssh_term_open
// ---------------------------------------------------------------------------

pub(super) fn ssh_term_open_tool(ctx: TermToolCtx) -> PortableDynamicTool {
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
                    "description": "可选。要启动的交互命令（如 python、top），会经过安全审查。auto 且有 tmux 时在新建 tmux 会话中启动；同名会话已存在时只 attach，不重复执行 command；off 时直接 PTY+exec"
                },
                "tmux": {
                    "type": "string",
                    "enum": ["auto", "required", "off"],
                    "description": "默认 auto：有 tmux 时 attach-or-create，未安装时开裸 PTY 并给安装引导。required：必须有 tmux，否则在执行 command 前报错；需保活/恢复/共屏的任务使用此模式。缺少 tmux 可显式调用 ssh_tmux_install，安装成功后使用 required 重开；open 不会自动安装。探测/启动失败报错。off 强制裸 PTY，不能与 tmuxSession 同用"
                },
                "tmuxSession": {
                    "type": "string",
                    "description": "可选。自定义 tmux 会话名，必须以 jkagent- 前缀开头（如 jkagent-install-nginx）。恢复时传回返回载荷 tmux_session 的值；此参数要求 tmux 可用，失败不会退回裸 shell。远端会话已不存在时同名新建，无法恢复已结束的进程"
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
        "打开交互式远程 PTY，返回 term_id、tmux_session、note 与首屏。默认 tmux=auto（含 command）：有 tmux 时新建或接入同名会话；断连后仍存活的会话可用 tmuxSession 恢复，人类可 tmux attach 共屏。无 tmux 的裸 PTY 不保证断连保活，也不能恢复现场，note 会明确提示；长任务需要保活时使用 tmux=required：缺少 tmux 会在执行前拒绝，可显式调用 ssh_tmux_install 经安全审查安装，再用 required 重开；open 本身不会安装。指定 tmuxSession 时禁止无 tmux 降级。配额：每服务器 ≤4、全局 ≤16。适用于 REPL、全屏程序、分步向导、确认提示；结构化命令与大脚本优先 ssh_exec。禁止通过终端输入密码/密钥/令牌，遇到凭据提示应改用非交互路径（如 ssh_exec sudo=true）或请用户介入。",
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
    let tmux = parse_tmux_preference(args)?;
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
                tmux,
                tmux_session,
            },
        )
        .await
        .map_err(map_term_error)?;

    let audit_command = open_audit_command(
        payload.tmux_session.as_deref(),
        args.get("command").and_then(Value::as_str),
        cols,
        rows,
    );
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

fn parse_tmux_preference(args: &Value) -> Result<TmuxPreference, ToolExecutionError> {
    match args.get("tmux") {
        None => Ok(TmuxPreference::Auto),
        Some(Value::String(mode)) if mode == "auto" => Ok(TmuxPreference::Auto),
        Some(Value::String(mode)) if mode == "required" => Ok(TmuxPreference::Required),
        Some(Value::String(mode)) if mode == "off" => Ok(TmuxPreference::Off),
        _ => Err(ToolExecutionError::invalid_args(
            "错误：tmux 只允许 auto、required 或 off",
        )),
    }
}

fn open_audit_command(
    tmux_session: Option<&str>,
    command: Option<&str>,
    cols: usize,
    rows: usize,
) -> String {
    let base = match tmux_session {
        // 保留审计回溯标记，级联清理依此回收跨重启的 tmux 会话。
        Some(name) => format!("ssh_term_open tmux new -A -s {name} ({cols}x{rows})"),
        None => format!("ssh_term_open shell ({cols}x{rows})"),
    };
    match command {
        Some(command) => format!("{base}; requested command: {command:?}"),
        None => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_tmux_preference() {
        for value in [json!("AUTO"), json!(false), Value::Null] {
            assert!(parse_tmux_preference(&json!({"tmux": value})).is_err());
        }
        assert!(matches!(
            parse_tmux_preference(&json!({})).unwrap(),
            TmuxPreference::Auto
        ));
        assert!(matches!(
            parse_tmux_preference(&json!({"tmux": "off"})).unwrap(),
            TmuxPreference::Off
        ));
    }

    #[test]
    fn tmux_command_audit_retains_session_for_cleanup() {
        let audit = open_audit_command(Some("jkagent-build"), Some("make all"), 80, 24);
        assert!(audit.starts_with("ssh_term_open tmux new -A -s jkagent-build "));
        assert!(audit.contains("make all"));
    }
}

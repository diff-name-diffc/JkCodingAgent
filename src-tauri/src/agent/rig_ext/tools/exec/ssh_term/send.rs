use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};

use super::input::parse_send_input;
use super::review::review_term_command;
use super::{append_term_audit, map_term_error, TermToolCtx};
use crate::agent::command_history::{self, CommandHistoryStatus};

// ---------------------------------------------------------------------------
// ssh_term_send
// ---------------------------------------------------------------------------

pub(super) fn ssh_term_send_tool(ctx: TermToolCtx) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "ssh_term_send",
        "向 ssh_term_open 打开的终端发送按键/文本，实际发送内容先经过安全审查。必填 intent（≤200 个 Unicode 字符，说明本次输入的目的）。默认 text_mode=escaped：字面转义只解析一次，\\u0003=Ctrl-C、\\u0004=Ctrl-D、\\u001b[A/B/C/D=方向键、\\r=CR、\\n=LF、\\t=Tab、\\e/\\x1b/\\033=ESC。text_mode=literal 保留反斜杠：输入 echo ready 并设 enter=true 即可执行，不要在末尾写字面 \\r。默认按键模式下，真实 ESC/TAB/LF/CR 仍会被远端 readline/TUI 当作按键，不能用它保证脚本原样传输。多行粘贴可显式设 paste=true：仅远端已启用 bracketed paste 时发送粘贴包络，未启用整次拒绝；配合 text_mode=literal 保留脚本反斜杠，enter=true 在包络外追加 CR 提交。复杂脚本或需要精确字节时使用 ssh_exec 的 stdin（如 command=\"bash -s\"），不经 readline。禁止发送任何密码/密钥/令牌——遇到密码提示应改用非交互路径（ssh_exec 的 sudo 参数）或请用户介入。包含粘贴包络与追加回车在内，最终发送文本上限 32000 个 Unicode 字符，超限整次拒绝、不截断。目标终端已退出时报错并附退出码。",
        json!({
            "type": "object",
            "properties": {
                "term_id": { "type": "string", "description": "ssh_term_open 返回的 term_id" },
                "text": {
                    "type": "string",
                    "description": "要发送的按键/文本。escaped 模式支持 \\uXXXX（含代理项对）、\\xXX（ASCII 00..7f）、1..3 位八进制转义（ASCII 000..177，如 \\033=ESC）、\\r/\\n/\\t/\\e/\\b/\\f/\\v/\\a 及转义的反斜杠/引号/斜杠；未知、残缺或非 ASCII 字节转义报错。literal 保留反斜杠。默认按键模式会把真实控制字符交给 readline/TUI 解释；paste=true 才使用粘贴包络，内容禁止包含包络起止序列。"
                },
                "text_mode": {
                    "type": "string",
                    "enum": ["escaped", "literal"],
                    "description": "默认 escaped：解析一次字面按键转义；literal：原样保留反斜杠，字面 \\r/\\n 永远不是回车。执行单行命令用 text=\"echo ready\"、text_mode=literal、enter=true；保留 shell 的 printf '\\033...' 转义也选 literal。精确脚本传输请使用 ssh_exec 的 stdin"
                },
                "paste": {
                    "type": "boolean",
                    "description": "默认 false（按键）。true：仅远端已开启 bracketed paste 时用 ESC[200~ / ESC[201~ 包裹解码后的文本；未开启或发送前已关闭则整次拒绝，不回退按键。推荐 text_mode=literal，多行内容中的换行不会由本工具单独提交；enter=true 在结束包络后追加 CR。这不是绕过远端程序的 raw-mode 通道"
                },
                "enter": {
                    "type": "boolean",
                    "description": "默认 false。true：按键模式在未以真实 CR/LF 结尾时追加 CR；paste=true 时始终在结束包络后追加 CR，提交粘贴内容。最终文本（含包络与 CR）计入 32000 字符上限并先经过安全审查，不改写 literal 中的字面 \\r/\\n"
                },
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
    let input = parse_send_input(args)?;
    let paste = input.paste;
    let (term_id, text, intent) = (input.term_id, input.text, input.intent);
    let Some(server_id) = ctx.registry.server_of(&term_id) else {
        return Err(map_term_error(crate::ssh_tool::term::TermError::NotFound(
            format!("终端会话 {term_id} 不存在，可先 ssh_term_list 查看"),
        )));
    };

    // 送审前先检查能力；送审结束后的实际写入还会再次检查，避免静默降级。
    if paste {
        ctx.registry
            .validate_paste_state(&term_id)
            .map_err(map_term_error)?;
    }
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
    // 通用审查构造器读取 compress_intent；明确传入本次按键目的。
    let review_args = json!({ "compress_intent": intent });
    let review = review_term_command(
        &ctx,
        "ssh_term_send",
        &server_id,
        &session_id,
        &text,
        Some(&review_args),
        screen_context,
    )
    .await?;

    if paste {
        ctx.registry.send_checked(&term_id, &text, true).await
    } else {
        ctx.registry.send(&term_id, &text).await
    }
    .map_err(map_term_error)?;
    append_term_audit(
        &ctx,
        &server_id,
        &session_id,
        // 完整 JSON 转义可逆：真实 CR 与字面 \\r 不混淆，也不丢弃中段输入。
        format!(
            "ssh_term_send {}",
            json!({ "text": text, "intent": intent, "paste": paste })
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

/// 控制字符可视化 + 头尾截断，仅用于短台账；完整内容另存审计。
fn summarize_text(text: &str) -> String {
    let visible: String = text
        .chars()
        .map(|c| match c {
            '\\' => "\\\\".to_string(),
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

//! 读屏参数与等待契约；等待只观察输出，后续输入仍走 send 审查。

use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};

use super::{current_cancel_rx, map_term_error, render_json, TermToolCtx};
use crate::agent::rig_ext::tools::common::{integer_u64, string_arg, with_compression_parameters};
use crate::ssh_tool::term::TermReadOptions;

pub(super) fn ssh_term_read_tool(ctx: TermToolCtx) -> PortableDynamicTool {
    let parameters = with_compression_parameters(
        json!({
            "type": "object",
            "properties": {
                "term_id": { "type": "string", "description": "ssh_term_open 返回的 term_id" },
                "wait_ms": {
                    "type": "integer", "minimum": 0, "maximum": 25000,
                    "description": "最长等待毫秒数。普通读取默认 0，有 wait_for 时默认 25000；超时只结束本次等待，不关闭终端"
                },
                "wait_for": {
                    "type": "string", "minLength": 1, "maxLength": 256,
                    "description": "等待当前屏幕或未读定稿行出现指定纯文本子串（大小写敏感，不是正则）。跨帧继续等待，返回 wait_status=matched/timed_out/exited/cancelled；取消等待时返回当前屏幕并省略尚未查询的 history；不会自动发送应答"
                },
                "include_ansi": {
                    "type": "boolean",
                    "description": "默认 false；true 附加 screen_ansi 样式快照，history.lines 同时保留 ANSI 样式；screen/new_lines 仍为纯文本"
                },
                "history_lines": {
                    "type": "integer", "minimum": 1, "maximum": 200,
                    "description": "可选：读取视口之前的历史，每页 1..200 行，单页约 12000 字节。tmux 从远端当前活动 pane capture-pane 读取（history.source=tmux），受远端 history-limit 限制；裸 PTY 从本地有限缓冲读取（source=terminal，最多 1000 行，备用屏无历史）"
                },
                "history_offset": {
                    "type": "integer", "minimum": 0,
                    "description": "与 history_lines 同用，跳过最近的 N 行历史，默认 0；用返回 history.next_offset 向更早翻页，每页按旧到新排列。新输出/resize 会改变相对偏移；tmux 历史随远端 pane 保留，裸 PTY 历史关闭即释放"
                }
            },
            "required": ["term_id"]
        }),
        false,
        crate::agent::rig_ext::tools::spec::DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
        "返回增量行、当前屏幕及可选历史/ANSI 快照。大屏输出可设 compress=true。",
    );
    PortableDynamicTool::new(
        "ssh_term_read",
        "读取终端屏幕与增量。new_lines_kind=completed_lines 表示主屏定稿行；screen_changes 表示 tmux/全屏两次读取间新增或改写的可见行，无法还原未观察到的历史。wait_for 提供 expect 式文本等待，匹配后再用 ssh_term_send 逐次审查应答；超时/取消只结束等待。history_lines 分页回溯 tmux 远端 pane 历史或裸 PTY 本地缓冲，不需要进入 copy-mode；include_ansi 保留颜色。awaiting_input/input_hint 是静默与当前提示符的启发式，false 不表示无需输入。exit_code 仅属于顶层进程，不代表 shell 内逐条命令；stdout/stderr 已合并。需要精确退出码或复杂脚本时使用 ssh_exec。禁止自动填入凭据或盲目接受主机指纹。",
        parameters,
        move |args| {
            let ctx = ctx.clone();
            Box::pin(async move {
                let term_id = string_arg(&args, "term_id").ok_or_else(|| invalid("缺少 term_id"))?;
                let options = parse_read_options(&args)?;
                let payload = ctx.registry.read(&term_id, &options, current_cancel_rx()).await.map_err(map_term_error)?;
                render_json(&payload).map(ToolOutput::text)
            })
        },
    )
}

fn invalid(message: &str) -> ToolExecutionError {
    ToolExecutionError::invalid_args(format!("错误：{message}"))
}

fn integer(args: &Value, key: &str, maximum: u64) -> Result<Option<u64>, ToolExecutionError> {
    args.get(key)
        .map(|value| {
            // 整值浮点兼容（如 80.0）对齐全仓 integer_u64 口径，不再只认 as_u64。
            integer_u64(value)
                .filter(|value| *value <= maximum)
                .ok_or_else(|| invalid(&format!("{key} 必须是 0..{maximum} 的整数")))
        })
        .transpose()
}

fn parse_read_options(args: &Value) -> Result<TermReadOptions, ToolExecutionError> {
    let wait_for = args
        .get("wait_for")
        .map(|value| {
            value
                .as_str()
                .filter(|s| !s.is_empty() && s.chars().count() <= 256)
                .map(str::to_owned)
                .ok_or_else(|| invalid("wait_for 必须是 1..256 字符的纯文本"))
        })
        .transpose()?;
    let wait_ms =
        integer(args, "wait_ms", 25_000)?.unwrap_or(if wait_for.is_some() { 25_000 } else { 0 });
    let include_ansi = args
        .get("include_ansi")
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| invalid("include_ansi 必须是布尔值"))
        })
        .transpose()?
        .unwrap_or(false);
    let history_lines = integer(args, "history_lines", 200)?;
    if history_lines == Some(0) {
        return Err(invalid("history_lines 必须是 1..200"));
    }
    let history_offset = integer(args, "history_offset", usize::MAX as u64)?;
    if history_offset.is_some() && history_lines.is_none() {
        return Err(invalid("history_offset 必须与 history_lines 同用"));
    }
    Ok(TermReadOptions {
        wait_ms,
        wait_for,
        include_ansi,
        history: history_lines.map(|limit| (history_offset.unwrap_or(0) as usize, limit as usize)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_options_before_touching_a_terminal() {
        for invalid in [
            json!({"wait_for":""}),
            json!({"wait_ms":-1}),
            json!({"wait_ms":25001}),
            json!({"include_ansi":"true"}),
            json!({"history_lines":0}),
            json!({"history_offset":1}),
        ] {
            assert!(parse_read_options(&invalid).is_err(), "{invalid}");
        }
        assert_eq!(
            parse_read_options(&json!({"wait_for":"Continue?"}))
                .unwrap()
                .wait_ms,
            25_000
        );
        let options = parse_read_options(
            &json!({"history_lines":50, "history_offset":100, "include_ansi":true}),
        )
        .unwrap();
        assert_eq!(options.history, Some((100, 50)));
        assert!(options.include_ansi);
    }
}

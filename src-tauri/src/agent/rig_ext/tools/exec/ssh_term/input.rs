//! 按键参数只解码一次；送审与发送必须复用同一个解码结果。

use std::str::Chars;

use rig::tool::ToolExecutionError;
use serde_json::Value;

use crate::agent::rig_ext::tools::common::string_arg;
use crate::ssh_tool::term::validate_send_text;

pub(super) struct SendInput {
    pub(super) term_id: String,
    pub(super) text: String,
    pub(super) intent: String,
    pub(super) paste: bool,
}

pub(super) fn parse_send_input(args: &Value) -> Result<SendInput, ToolExecutionError> {
    let term_id = string_arg(args, "term_id").ok_or_else(|| {
        ToolExecutionError::invalid_args("错误：缺少必填参数 term_id。".to_string())
    })?;
    // text 不得 trim：空格、换行、Tab 和控制字符都是有效按键。
    let source = string_arg(args, "text")
        .ok_or_else(|| ToolExecutionError::invalid_args("错误：缺少必填参数 text。".to_string()))?;
    let intent = string_arg(args, "intent")
        .filter(|intent| !intent.trim().is_empty())
        .ok_or_else(|| {
            ToolExecutionError::invalid_args(
                "错误：缺少必填参数 intent（审查与审计依赖它判断按键目的）。".to_string(),
            )
        })?;
    if intent.chars().count() > 200 {
        return Err(ToolExecutionError::invalid_args(
            "错误：intent 不能超过 200 字符。".to_string(),
        ));
    }
    let mut text = match args.get("text_mode") {
        None => decode_escapes(&source).map_err(ToolExecutionError::invalid_args)?,
        Some(Value::String(mode)) if mode == "escaped" => {
            decode_escapes(&source).map_err(ToolExecutionError::invalid_args)?
        }
        Some(Value::String(mode)) if mode == "literal" => source,
        _ => {
            return Err(ToolExecutionError::invalid_args(
                "错误：text_mode 只能是 escaped 或 literal。".to_string(),
            ));
        }
    };
    let enter = parse_strict_bool(args, "enter")?;
    let paste = parse_strict_bool(args, "paste")?;
    if paste {
        // 禁止载荷提前关闭或嵌套粘贴包络，避免后续文本变成独立按键。
        if ["\u{1b}[200~", "\u{1b}[201~", "\u{9b}200~", "\u{9b}201~"]
            .iter()
            .any(|marker| text.contains(marker))
        {
            return Err(ToolExecutionError::invalid_args(
                "错误：paste 文本不能包含 bracketed paste 起止序列；本次未发送。需要原样传送脚本字节请使用 ssh_exec 的 stdin。".to_string(),
            ));
        }
        text = format!("\u{1b}[200~{text}\u{1b}[201~");
    }
    // paste 内的末尾换行属于内容，提交用的 CR 必须在结束包络之后发送。
    if enter && (paste || !text.ends_with(['\r', '\n'])) {
        text.push('\r');
    }
    // 包络与附加回车也计入上限；送审与发送使用相同字节，超长整次拒绝。
    validate_send_text(&text)
        .map_err(|error| ToolExecutionError::invalid_args(format!("错误：{error}")))?;
    Ok(SendInput {
        term_id,
        text,
        intent,
        paste,
    })
}

/// 布尔参数严格校验：缺省 false；非布尔值报错而非静默当作 false
/// （send 的 enter/paste 与 close 的 killTmuxSession 共用同一口径）。
pub(super) fn parse_strict_bool(args: &Value, key: &str) -> Result<bool, ToolExecutionError> {
    Ok(match args.get(key) {
        None => false,
        Some(Value::Bool(value)) => *value,
        _ => {
            return Err(ToolExecutionError::invalid_args(format!(
                "错误：{key} 只能是布尔值。"
            )));
        }
    })
}

fn invalid_escape(reason: &str) -> String {
    format!("错误：text 转义无效：{reason}。需要保留反斜杠时请使用 text_mode=literal；真实控制字符仍会被远端程序解释，复杂脚本请使用 ssh_exec 或 ssh_term_open 的 command。")
}

fn read_hex(chars: &mut Chars<'_>, digits: usize) -> Result<u32, String> {
    (0..digits).try_fold(0, |value, _| {
        chars
            .next()
            .and_then(|c| c.to_digit(16))
            .map(|digit| value * 16 + digit)
            .ok_or_else(|| invalid_escape("十六进制位数不足或包含非十六进制字符"))
    })
}

fn read_unicode_escape(chars: &mut Chars<'_>) -> Result<char, String> {
    let first = read_hex(chars, 4)?;
    let scalar = if (0xd800..=0xdbff).contains(&first) {
        if chars.next() != Some('\\') || chars.next() != Some('u') {
            return Err(invalid_escape("Unicode 高代理项后必须紧跟低代理项 \\uXXXX"));
        }
        let second = read_hex(chars, 4)?;
        if !(0xdc00..=0xdfff).contains(&second) {
            return Err(invalid_escape("Unicode 代理项配对无效"));
        }
        0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00)
    } else {
        first
    };
    char::from_u32(scalar).ok_or_else(|| invalid_escape("不是有效的 Unicode 字符"))
}

fn read_octal_escape(first: char, chars: &mut Chars<'_>) -> Result<char, String> {
    let mut byte = first as u32 - '0' as u32;
    for _ in 0..2 {
        let Some(digit) = chars.clone().next().and_then(|c| c.to_digit(8)) else {
            break;
        };
        chars.next();
        byte = byte * 8 + digit;
    }
    if byte > 0x7f {
        return Err(invalid_escape(
            "八进制转义仅支持 ASCII 000..177；非 ASCII 文本请直接输入 Unicode 或使用 \\uXXXX",
        ));
    }
    Ok(char::from(byte as u8))
}

fn decode_escapes(source: &str) -> Result<String, String> {
    let mut chars = source.chars();
    let mut decoded = String::with_capacity(source.len());
    while let Some(c) = chars.next() {
        if c != '\\' {
            decoded.push(c);
            continue;
        }
        let escaped = chars
            .next()
            .ok_or_else(|| invalid_escape("末尾反斜杠缺少转义字符"))?;
        let value = match escaped {
            '\\' | '"' | '\'' | '/' => escaped,
            '0'..='7' => read_octal_escape(escaped, &mut chars)?,
            'a' => '\u{7}',
            'b' => '\u{8}',
            't' => '\t',
            'n' => '\n',
            'v' => '\u{b}',
            'f' => '\u{c}',
            'r' => '\r',
            'e' => '\u{1b}',
            'u' => read_unicode_escape(&mut chars)?,
            'x' => {
                let byte = read_hex(&mut chars, 2)?;
                if byte > 0x7f {
                    return Err(invalid_escape(
                        "\\xXX 仅支持 ASCII 字节 00..7f；非 ASCII 文本请直接输入 Unicode 或使用 \\uXXXX",
                    ));
                }
                char::from(byte as u8)
            }
            _ => return Err(invalid_escape(&format!("不支持 \\{escaped}"))),
        };
        // 只扫描原始输入；由 \\ 或 \u005c 产生的反斜杠不再参与解码。
        decoded.push(value);
    }
    Ok(decoded)
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;

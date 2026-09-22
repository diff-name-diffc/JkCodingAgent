use anyhow::Result;

pub(crate) type CommandResult<T> = std::result::Result<T, String>;

pub(crate) trait IntoCommandResult<T> {
    fn into_command_result(self) -> CommandResult<T>;
}

impl<T> IntoCommandResult<T> for Result<T> {
    fn into_command_result(self) -> CommandResult<T> {
        self.map_err(|error| format_anyhow_error(&error))
    }
}

/// 把 anyhow 错误链格式化为用户可读文案。
///
/// `{error}` / `Error::to_string()` 只保留最外层 context，会丢掉 HTTP 状态、
/// 响应体、连接失败等根因；`{error:#}` 用 `: ` 拼成一行，长链在聊天里难扫。
/// 这里按「首行摘要 + 原因列表」展开，供 Failed 事件与命令 Result 共用。
pub(crate) fn format_anyhow_error(error: &anyhow::Error) -> String {
    let mut chain = error.chain();
    let Some(top) = chain.next() else {
        return String::new();
    };

    let mut lines = vec![top.to_string()];
    for cause in chain {
        let text = cause.to_string();
        if lines
            .last()
            .is_some_and(|previous| previous.contains(&text))
        {
            continue;
        }
        lines.push(text);
    }

    if lines.len() == 1 {
        return lines.remove(0);
    }

    let mut out = lines[0].clone();
    out.push_str("\n原因：");
    out.push_str(&lines[1]);
    for extra in &lines[2..] {
        out.push('\n');
        out.push_str(extra);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::format_anyhow_error;
    use anyhow::anyhow;

    #[test]
    fn single_error_is_plain_message() {
        let error = anyhow!("boom");
        assert_eq!(format_anyhow_error(&error), "boom");
    }

    #[test]
    fn nested_context_lists_causes_on_separate_lines() {
        let error = anyhow!("connection refused")
            .context("发送流式对话请求失败：model=qwen url=http://localhost/v1/chat/completions")
            .context("LLM 流式请求失败");
        let message = format_anyhow_error(&error);
        assert!(
            message.starts_with(
                "LLM 流式请求失败\n原因：发送流式对话请求失败：model=qwen url=http://localhost/v1/chat/completions"
            ),
            "{message}"
        );
        assert!(message.contains("connection refused"), "{message}");
        assert!(
            !message.contains("LLM 流式请求失败: "),
            "must not collapse the chain into a single Display line: {message}"
        );
    }

    #[test]
    fn skips_cause_already_contained_in_previous_line() {
        let error = anyhow!("connection refused")
            .context("tcp connect error: connection refused")
            .context("LLM 流式请求失败");
        let message = format_anyhow_error(&error);
        assert_eq!(
            message,
            "LLM 流式请求失败\n原因：tcp connect error: connection refused"
        );
    }
}

//! 子智能体上下文预算与滑动窗口裁剪（rig `Message` 形态）。
//!
//! 迁移自旧 `agent/sub_agent/runtime/context.rs`；「轮次」定义不变
//! （一条 assistant 消息 + 紧随其后的全部工具响应，不可拆分，避免产生孤儿
//! 工具消息），字符预算仍由模型库条目 `contextWindow` 派生。

use rig::completion::Message;
use rig::message::{AssistantContent, UserContent};

/// 上下文裁剪字符预算：统一容量源（模型库条目 contextWindow，未配置回退 1M）
/// × 4 字符/token × 1/2——窗口的一半留给系统提示、可见输出与单轮工具结果。
pub(crate) fn context_budget_chars(context_window: Option<u64>) -> usize {
    const CHARS_PER_TOKEN: u64 = 4;
    let window_tokens = context_window
        .unwrap_or(crate::agent::db::DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS);
    (window_tokens * CHARS_PER_TOKEN / 2) as usize
}

/// 估算单条消息的上下文占用（字符数）：正文 + 思考 + 工具调用参数/结果。
pub(crate) fn message_chars(message: &Message) -> usize {
    match message {
        Message::System { content } => content.chars().count(),
        Message::Assistant { content, .. } => content
            .iter()
            .map(|item| match item {
                AssistantContent::Text(text) => text.text.chars().count(),
                AssistantContent::Reasoning(reasoning) => {
                    reasoning.display_text().chars().count()
                }
                AssistantContent::ToolCall(call) => {
                    call.function.name.chars().count()
                        + call.function.arguments.to_string().chars().count()
                }
                AssistantContent::Image(_) => 0,
            })
            .sum(),
        Message::User { content } => content
            .iter()
            .map(|item| match item {
                UserContent::Text(text) => text.text.chars().count(),
                UserContent::ToolResult(result) => result
                    .content
                    .iter()
                    .map(|block| block.as_text().map(str::len).unwrap_or(0))
                    .sum(),
                _ => 0,
            })
            .sum(),
    }
}

/// 滑动窗口裁剪（纯函数）：保留头部两条消息（system + 首轮 user）与最近
/// 若干轮完整对话；中间轮次整体移除并插入占位说明，避免模型误以为任务刚开始。
/// 返回 None 表示无需裁剪（调用方据此跳过整份历史 clone）。
pub(crate) fn trim_context_messages(
    messages: &[Message],
    max_chars: usize,
    keep_recent_rounds: usize,
) -> Option<Vec<Message>> {
    const HEADER_LEN: usize = 2; // system + 首轮 user
    if messages.len() <= HEADER_LEN || keep_recent_rounds == 0 {
        return None;
    }
    let (header, rest) = messages.split_at(HEADER_LEN);

    // 以 assistant 消息为起点切分轮次。
    let mut rounds: Vec<&[Message]> = Vec::new();
    let mut start = 0usize;
    for (index, message) in rest.iter().enumerate() {
        if matches!(message, Message::Assistant { .. }) && index > start {
            rounds.push(&rest[start..index]);
            start = index;
        }
    }
    rounds.push(&rest[start..]);

    // 从最后一轮向前选择保留窗口：轮数与字符数双重约束，至少保留一轮。
    let mut kept_chars = 0usize;
    let mut keep_from = rounds.len();
    for (index, round) in rounds.iter().enumerate().rev() {
        let kept_count = rounds.len() - keep_from;
        if kept_count >= keep_recent_rounds {
            break;
        }
        let round_chars: usize = round.iter().map(message_chars).sum();
        if kept_count > 0 && kept_chars.saturating_add(round_chars) > max_chars {
            break;
        }
        keep_from = index;
        kept_chars = kept_chars.saturating_add(round_chars);
    }

    if keep_from == 0 {
        return None;
    }

    let dropped_messages: usize = rounds[..keep_from].iter().map(|round| round.len()).sum();
    let mut trimmed = Vec::with_capacity(messages.len() - dropped_messages + 1);
    trimmed.extend_from_slice(header);
    trimmed.push(Message::User {
        content: vec![UserContent::text(format!(
            "【上下文裁剪】因上下文长度限制，此前 {dropped_messages} 条工具调用相关消息已被省略。如需其中的信息，请重新调用相应工具获取。"
        ))],
    });
    for round in &rounds[keep_from..] {
        trimmed.extend_from_slice(round);
    }
    Some(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig::completion::Message;

    fn user(text: &str) -> Message {
        Message::user(text)
    }

    fn assistant_with_tool() -> Message {
        Message::Assistant {
            id: None,
            content: vec![AssistantContent::ToolCall(rig::message::ToolCall::from_wire(
                "call-1",
                rig::message::ToolFunction::new("read_file".to_string(), serde_json::json!({"path": "a"})),
            ))],
        }
    }

    fn tool_result(text: &str) -> Message {
        Message::User {
            content: vec![UserContent::ToolResult(rig::message::ToolResult {
                call: rig::message::ToolCallId::for_provider(None),
                provider: None,
                name: "read_file".to_string(),
                content: vec![rig::message::ToolResultContent::text(text)],
            })],
        }
    }

    #[test]
    fn header_and_last_round_survive_trimming() {
        let mut messages = vec![
            Message::system("系统提示"),
            user("首轮用户任务"),
            assistant_with_tool(),
            tool_result("r1"),
            user("补充"),
            assistant_with_tool(),
            tool_result("r2"),
        ];
        // 预算极小：只保留最后一轮。
        let trimmed = trim_context_messages(&messages, 1, 200).expect("需要裁剪");
        assert!(matches!(trimmed[0], Message::System { .. }));
        assert!(matches!(trimmed[1], Message::User { .. }));
        // 占位说明位于头部之后。
        let Message::User { content } = &trimmed[2] else {
            panic!("应插入占位说明");
        };
        assert!(format!("{content:?}").contains("上下文裁剪"));
        assert!(trimmed.len() < messages.len());

        // 预算充足时不裁剪。
        messages.truncate(7);
        assert!(trim_context_messages(&messages, usize::MAX, 200).is_none());
    }

    #[test]
    fn budget_drives_window_when_window_is_configured() {
        assert_eq!(context_budget_chars(Some(1_000_000)) , 2_000_000);
        assert_eq!(context_budget_chars(None), 2_000_000);
    }
}

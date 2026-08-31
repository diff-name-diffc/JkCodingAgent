use super::*;

/// G13-05 升级决策（纯函数）：任一失败工具在此前轮次已失败过
/// （按工具名记录的失败轮数 ≥ 1，即已消耗重试资格）时升级为强制收口。
/// 同一轮内同名工具的多次失败（并行批次）在计数时只记一轮，
/// 因此不会被误判为「重试后仍失败」。
pub(super) fn should_escalate_tool_failures(
    failed_tool_names: &[String],
    tool_failure_rounds: &HashMap<String, u32>,
) -> bool {
    failed_tool_names
        .iter()
        .any(|name| tool_failure_rounds.get(name).copied().unwrap_or(0) >= 1)
}

/// 估算单条消息的上下文占用（字符数）：content + reasoning + 工具调用参数。
pub(super) fn chat_message_chars(message: &ChatMessage) -> usize {
    let mut chars = message.content.chars().count();
    if let Some(reasoning) = &message.reasoning_content {
        chars += reasoning.chars().count();
    }
    if let Some(tool_calls) = &message.tool_calls {
        for tc in tool_calls {
            chars += tc.function.name.chars().count() + tc.function.arguments.chars().count();
        }
    }
    chars
}

/// G13-07：消息历史滑动窗口裁剪（纯函数）。
///
/// 保留头部两条消息（system + 首轮 user）与最近若干轮完整对话；
/// 中间轮次整体移除，并插入一条占位说明，避免模型误以为任务刚开始。
/// 「轮次」= 一条 assistant 消息 + 紧随其后的全部 tool 响应，是不可拆分的
/// 最小单元——只按轮次边界裁剪才不会产生孤儿 tool 消息，保证发往
/// OpenAI 兼容接口的消息序列始终合法。单条工具结果在写入时已按
/// SUB_AGENT_RESULT_MAX_CHARS 截断，本函数在其上约束上下文总量：
/// 最近轮次数量与总字符数任一超限即收紧窗口，但至少保留最后一轮。
///
/// 返回 None 表示无需裁剪——调用方据此跳过整份历史的 clone。
pub(super) fn trim_context_messages(
    messages: &[ChatMessage],
    max_chars: usize,
    keep_recent_rounds: usize,
) -> Option<Vec<ChatMessage>> {
    const HEADER_LEN: usize = 2; // system + 首轮 user
    if messages.len() <= HEADER_LEN || keep_recent_rounds == 0 {
        return None;
    }
    let (header, rest) = messages.split_at(HEADER_LEN);

    // 以 assistant 消息为起点切分轮次。
    let mut rounds: Vec<&[ChatMessage]> = Vec::new();
    let mut start = 0usize;
    for (index, message) in rest.iter().enumerate() {
        if message.role == "assistant" && index > start {
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
        let round_chars: usize = round.iter().map(chat_message_chars).sum();
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
    trimmed.push(ChatMessage {
        role: "user".to_string(),
        content: format!(
            "【上下文裁剪】因上下文长度限制，此前 {dropped_messages} 条工具调用相关消息已被省略。如需其中的信息，请重新调用相应工具获取。"
        ),
        content_parts: Vec::new(),
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
    });
    for round in &rounds[keep_from..] {
        trimmed.extend_from_slice(round);
    }
    Some(trimmed)
}

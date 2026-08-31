use super::*;
use serde_json::json;

fn msg(role: &str, content: &str) -> ChatMessage {
    ChatMessage {
        role: role.to_string(),
        content: content.to_string(),
        content_parts: Vec::new(),
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
    }
}

#[test]
fn trace_collector_merges_adjacent_llm_deltas_and_keeps_timestamps() {
    let trace = Arc::new(Mutex::new(Vec::new()));
    record_trace_event(
        &trace,
        json!({"event":"llmDelta","data":{"delta":"你"}}),
        10,
    );
    record_trace_event(
        &trace,
        json!({"event":"llmDelta","data":{"delta":"好"}}),
        11,
    );
    record_trace_event(
        &trace,
        json!({"event":"UsageUpdated","data":{"elapsedMs":20}}),
        20,
    );

    let events = trace.lock();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["data"]["delta"], "你好");
    assert_eq!(events[0]["timestampMs"], 10);
    assert_eq!(events[1]["timestampMs"], 20);
}

#[test]
fn trace_events_are_capped_with_visible_truncation_marker() {
    // G1-20：超限时丢弃最旧事件，头部 traceTruncated 标记记录累计丢弃数。
    let trace = Arc::new(Mutex::new(Vec::new()));
    let total = SUB_AGENT_TRACE_EVENT_LIMIT + 50;
    for index in 0..total {
        record_trace_event(
            &trace,
            json!({"event":"ToolStarted","data":{"index": index}}),
            index as i64,
        );
    }

    let events = trace.lock();
    assert_eq!(events.len(), SUB_AGENT_TRACE_EVENT_LIMIT);
    assert_eq!(events[0]["event"], TRACE_TRUNCATED_EVENT);
    // 丢弃 50 条超限事件 + 1 条为标记腾位 = 51。
    assert_eq!(events[0]["data"]["dropped"], 51);
    // 最新事件必须保留。
    assert_eq!(events.last().unwrap()["data"]["index"], (total - 1) as u64);
}

#[test]
fn escalation_requires_a_previous_failed_round_for_the_same_tool() {
    // G13-05：按工具名记录重试资格。
    let mut rounds: HashMap<String, u32> = HashMap::new();

    // 首次失败：不升级，允许重试。
    assert!(!should_escalate_tool_failures(
        &["read_file".to_string()],
        &rounds
    ));

    // 同名工具此前轮次已失败过：升级强制收口。
    rounds.insert("read_file".to_string(), 1);
    assert!(should_escalate_tool_failures(
        &["read_file".to_string()],
        &rounds
    ));

    // 其他工具的首次失败不受牵连（修复原全局标志的交叉污染）。
    assert!(!should_escalate_tool_failures(
        &["search".to_string()],
        &rounds
    ));

    // 无失败工具：不升级。
    assert!(!should_escalate_tool_failures(&[], &rounds));
}

#[test]
fn trim_context_keeps_header_and_recent_rounds_only() {
    // 头部（system + 首轮 user）+ 三轮 assistant/tool 对话。
    let messages = vec![
        msg("system", "sys"),
        msg("user", "task"),
        msg("assistant", "a1"),
        msg("tool", "t1"),
        msg("assistant", "a2"),
        msg("tool", "t2"),
        msg("assistant", "a3"),
        msg("tool", "t3"),
    ];

    let trimmed = trim_context_messages(&messages, 120_000, 1).expect("应当触发裁剪");
    // 头部 2 条 + 占位说明 1 条 + 最后一轮 2 条。
    assert_eq!(trimmed.len(), 5);
    assert_eq!(trimmed[0].content, "sys");
    assert_eq!(trimmed[1].content, "task");
    assert!(trimmed[2].content.contains("上下文裁剪"));
    // 保留的轮次必须完整（assistant + 其 tool 响应），无孤儿消息。
    assert_eq!(trimmed[3].role, "assistant");
    assert_eq!(trimmed[3].content, "a3");
    assert_eq!(trimmed[4].role, "tool");
    assert_eq!(trimmed[4].content, "t3");
}

#[test]
fn trim_context_noop_when_within_limits() {
    let messages = vec![
        msg("system", "sys"),
        msg("user", "task"),
        msg("assistant", "a1"),
        msg("tool", "t1"),
    ];
    // 无需裁剪时返回 None，调用方跳过全量 clone。
    assert!(trim_context_messages(&messages, 120_000, 8).is_none());
}

#[test]
fn trim_context_shrinks_window_under_char_budget() {
    // 三轮各约 50k 字符；窗口上限 120k ⇒ 只能保留最近两轮。
    let big: String = "测".repeat(50_000);
    let messages = vec![
        msg("system", "sys"),
        msg("user", "task"),
        msg("assistant", &big),
        msg("tool", "t1"),
        msg("assistant", &big),
        msg("tool", "t2"),
        msg("assistant", &big),
        msg("tool", "t3"),
    ];
    let trimmed = trim_context_messages(&messages, 120_000, 8).expect("应当触发裁剪");
    // 头部 2 + 占位 1 + 两轮 4 = 7；第一轮被裁剪。
    assert_eq!(trimmed.len(), 7);
    assert!(trimmed[2].content.contains("上下文裁剪"));
    assert!(!trimmed.iter().any(|m| m.content == "t1"));
}

#[test]
fn trim_context_always_keeps_at_least_one_round() {
    // 单轮即超预算时仍保留最后一轮（最近的上下文最关键）⇒ 无需裁剪。
    let big: String = "测".repeat(200_000);
    let messages = vec![
        msg("system", "sys"),
        msg("user", "task"),
        msg("assistant", &big),
        msg("tool", "t1"),
    ];
    assert!(trim_context_messages(&messages, 120_000, 8).is_none());
}

use serde_json::json;

use crate::agent::llm::{ChatMessage, FunctionCall, OutboundToolCall};

use super::{
    cancellation_requested, classify_tool_result, prepare_tool_result, should_keep_llm_message,
    ToolOutcome, TOOL_RESULT_INLINE_MAX_CHARS_PAGED, TOOL_RESULT_INLINE_MAX_CHARS_READ,
};

#[test]
fn dropped_cancellation_sender_is_fail_closed() {
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    assert!(!cancellation_requested(&cancel_rx));
    drop(cancel_tx);
    assert!(cancellation_requested(&cancel_rx));
}

fn chat_message(role: &str, content: &str) -> ChatMessage {
    ChatMessage {
        reasoning_content: None,
        role: role.to_string(),
        content: content.to_string(),
        content_parts: Vec::new(),
        tool_call_id: None,
        name: None,
        tool_calls: None,
    }
}

#[test]
fn keeps_normal_user_assistant_and_tool_messages() {
    assert!(should_keep_llm_message(&chat_message(
        "user",
        "帮我查一下天气"
    )));
    assert!(should_keep_llm_message(&chat_message(
        "assistant",
        "已经为你查询了天气"
    )));
    let mut tool_result = chat_message("tool", "北京今天晴");
    tool_result.name = Some("browser_read_text".to_string());
    assert!(should_keep_llm_message(&tool_result));
}

#[test]
fn filters_dispatch_plumbing_tool_results() {
    let mut tool_result = chat_message("tool", "claude 子进程输出...");
    tool_result.name = Some("dispatch_claude".to_string());
    assert!(!should_keep_llm_message(&tool_result));
}

#[test]
fn filters_process_only_assistant_messages() {
    assert!(!should_keep_llm_message(&chat_message(
        "assistant",
        "✅ 子任务进程已结束"
    )));
    assert!(!should_keep_llm_message(&chat_message(
        "assistant",
        "📋 已提交 执行图，等待确认"
    )));
}

#[test]
fn filters_assistant_message_that_only_makes_plumbing_tool_calls() {
    let mut message = chat_message("assistant", "");
    message.tool_calls = Some(vec![OutboundToolCall {
        id: "call_1".to_string(),
        kind: "function".to_string(),
        function: FunctionCall {
            name: "dispatch_claude".to_string(),
            arguments: "{}".to_string(),
        },
    }]);
    assert!(!should_keep_llm_message(&message));

    // 混合了非 plumbing 工具调用时保留（与 DB 加载口径一致）
    message.tool_calls.as_mut().unwrap().push(OutboundToolCall {
        id: "call_2".to_string(),
        kind: "function".to_string(),
        function: FunctionCall {
            name: "read_file_content".to_string(),
            arguments: "{}".to_string(),
        },
    });
    assert!(should_keep_llm_message(&message));
}

#[test]
fn classifies_tool_error_as_recoverable_without_matching_specific_text() {
    assert_eq!(
        classify_tool_result("错误：任意工具错误都应先交回模型修正"),
        ToolOutcome::RecoverableError {
            message: "错误：任意工具错误都应先交回模型修正".to_string()
        }
    );
}

#[test]
fn compress_false_truncates_without_requesting_summary() {
    let raw = (1..=1_000)
        .map(|line| format!("{line}|0123456789"))
        .collect::<Vec<_>>()
        .join("\n");

    let prepared = prepare_tool_result("read_file", &json!({ "compress": false }), &raw);

    assert!(!prepared.needs_summary);
    assert_eq!(prepared.result_mode, "truncated");
    assert!(prepared.display_content.contains("结果已截断"));
    assert!(prepared.display_content.contains("截断发生在原始结果第"));
    assert!(prepared
        .display_content
        .contains("该行标注的源码/匹配行号为"));
    assert_eq!(
        prepared
            .display_content
            .split("\n\n[")
            .next()
            .unwrap_or_default()
            .chars()
            .count(),
        TOOL_RESULT_INLINE_MAX_CHARS_READ
    );
    assert_eq!(prepared.raw_output, raw);
}

#[test]
fn compress_true_summarizes_only_above_five_thousand_characters() {
    let medium = "x".repeat(5_000);
    let large = "x".repeat(5_001);

    let medium_prepared = prepare_tool_result("grep", &json!({ "compress": true }), &medium);
    let large_prepared = prepare_tool_result("grep", &json!({ "compress": true }), &large);

    assert!(!medium_prepared.needs_summary);
    // grep 属读取类工具，5000 字符在 10000 内联预算内，完整内联返回。
    assert_eq!(medium_prepared.result_mode, "raw");
    assert!(large_prepared.needs_summary);
    assert_eq!(large_prepared.result_mode, "pending_summary");
}

#[test]
fn compress_false_never_summarizes_large_results() {
    let raw = "x".repeat(TOOL_RESULT_INLINE_MAX_CHARS_READ + 1_000);

    let prepared = prepare_tool_result("grep", &json!({ "compress": false }), &raw);

    assert!(!prepared.needs_summary);
    assert_eq!(prepared.result_mode, "truncated");
}

#[test]
fn explicit_line_range_raises_browser_read_text_inline_budget() {
    let raw = (1..=3_000)
        .map(|line| format!("{line}|0123456789"))
        .collect::<Vec<_>>()
        .join("\n");

    let paged = prepare_tool_result(
        "browser_read_text",
        &json!({ "compress": false, "offset": 51, "limit": 200 }),
        &raw,
    );

    assert!(!paged.needs_summary);
    assert_eq!(paged.result_mode, "truncated");
    assert!(paged.display_content.contains("仅返回前 20000 /"));
    assert_eq!(
        paged
            .display_content
            .split("\n\n[")
            .next()
            .unwrap_or_default()
            .chars()
            .count(),
        TOOL_RESULT_INLINE_MAX_CHARS_PAGED
    );
    assert_eq!(paged.raw_output, raw);
}

#[test]
fn read_file_explicit_line_range_also_raises_inline_budget() {
    let raw = "x".repeat(30_000);

    let paged = prepare_tool_result(
        "read_file",
        &json!({ "compress": false, "offset": 101, "limit": 500 }),
        &raw,
    );

    assert_eq!(paged.result_mode, "truncated");
    assert!(paged.display_content.contains("仅返回前 20000 /"));
}

#[test]
fn browser_read_text_without_explicit_range_uses_read_budget() {
    let raw = "x".repeat(TOOL_RESULT_INLINE_MAX_CHARS_READ + 1);

    let prepared = prepare_tool_result("browser_read_text", &json!({ "compress": false }), &raw);

    assert_eq!(prepared.result_mode, "truncated");
    assert!(prepared.display_content.contains("仅返回前 10000 /"));
}

#[test]
fn read_tools_without_paging_params_use_read_budget() {
    for tool_name in ["grep", "glob", "list_dir", "graph_plan_report"] {
        let raw = "x".repeat(TOOL_RESULT_INLINE_MAX_CHARS_READ + 1);

        let prepared = prepare_tool_result(tool_name, &json!({ "compress": false }), &raw);

        assert_eq!(prepared.result_mode, "truncated", "{tool_name}");
        assert!(
            prepared.display_content.contains("仅返回前 10000 /"),
            "{tool_name}"
        );
    }
}

#[test]
fn offset_on_non_read_tools_keeps_default_budget() {
    let raw = "x".repeat(9_000);

    let prepared = prepare_tool_result(
        "exec",
        &json!({ "compress": false, "offset": 51, "limit": 200 }),
        &raw,
    );

    assert_eq!(prepared.result_mode, "truncated");
    assert!(prepared.display_content.contains("仅返回前 8000 /"));
}

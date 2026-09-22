use crate::agent::llm::{ChatMessage, FunctionCall, OutboundToolCall};

use super::{cancellation_requested, classify_tool_result, should_keep_llm_message, ToolOutcome};

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

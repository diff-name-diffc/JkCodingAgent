use crate::agent::db::{ChatMessage, FunctionCall, OutboundToolCall};

use super::{cancellation_requested, repair_tool_call_pairing, should_keep_llm_message};

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
        source_id: None,
    }
}

fn assistant_with_tool_calls(content: &str, calls: &[(&str, &str)]) -> ChatMessage {
    let mut message = chat_message("assistant", content);
    message.tool_calls = Some(
        calls
            .iter()
            .map(|(id, name)| OutboundToolCall {
                id: (*id).to_string(),
                kind: "function".to_string(),
                function: FunctionCall {
                    name: (*name).to_string(),
                    arguments: "{}".to_string(),
                },
            })
            .collect(),
    );
    message
}

fn tool_result(tool_call_id: &str, name: &str, content: &str) -> ChatMessage {
    let mut message = chat_message("tool", content);
    message.tool_call_id = Some(tool_call_id.to_string());
    message.name = Some(name.to_string());
    message
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
fn repair_keeps_healthy_pairing_untouched() {
    let mut messages = vec![
        chat_message("user", "帮我跑两个命令"),
        assistant_with_tool_calls(
            "并行执行",
            &[("call-a", "ssh_exec"), ("call-b", "ssh_exec")],
        ),
        tool_result("call-a", "ssh_exec", "结果A"),
        tool_result("call-b", "ssh_exec", "结果B"),
        chat_message("assistant", "完成"),
    ];
    let expected = messages
        .iter()
        .map(|message| (message.role.clone(), message.tool_call_id.clone()))
        .collect::<Vec<_>>();

    repair_tool_call_pairing(&mut messages);

    let actual = messages
        .iter()
        .map(|message| (message.role.clone(), message.tool_call_id.clone()))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}

#[test]
fn repair_fills_missing_tool_results_in_call_order() {
    let mut messages = vec![
        chat_message("user", "帮我跑两个命令"),
        assistant_with_tool_calls(
            "并行执行",
            &[("call-a", "ssh_exec"), ("call-b", "ssh_exec")],
        ),
        // 批量中途取消：只落了第一个结果
        tool_result("call-a", "ssh_exec", "结果A"),
        chat_message("user", "继续"),
    ];

    repair_tool_call_pairing(&mut messages);

    let shape = messages
        .iter()
        .map(|message| {
            (
                message.role.as_str(),
                message.tool_call_id.as_deref().unwrap_or("-"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        shape,
        vec![
            ("user", "-"),
            ("assistant", "-"),
            ("tool", "call-a"),
            ("tool", "call-b"),
            ("user", "-"),
        ]
    );
    let placeholder = &messages[3];
    assert_eq!(placeholder.name.as_deref(), Some("ssh_exec"));
    assert!(
        placeholder.content.contains("没有产生结果"),
        "占位结果应说明未产生结果：{}",
        placeholder.content
    );
}

#[test]
fn repair_drops_tool_results_without_a_preceding_call() {
    let mut messages = vec![
        // 孤儿结果（其 assistant 被上下文过滤丢弃）：无处应答，必须剔除
        tool_result("call-orphan", "dispatch_claude", "孤儿输出"),
        chat_message("user", "你好"),
        assistant_with_tool_calls("查询", &[("call-a", "read_file_content")]),
        tool_result("call-a", "read_file_content", "内容"),
        // 未被任何 tool_calls 声明的多余结果：同样剔除
        tool_result("call-extra", "ssh_exec", "多余"),
    ];

    repair_tool_call_pairing(&mut messages);

    let shape = messages
        .iter()
        .map(|message| {
            (
                message.role.as_str(),
                message.tool_call_id.as_deref().unwrap_or("-"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        shape,
        vec![("user", "-"), ("assistant", "-"), ("tool", "call-a"),]
    );
}

#[test]
fn repair_matches_results_by_id_regardless_of_position() {
    // 结果顺序与 tool_calls 顺序不一致时，按调用顺序重排，保持可应答。
    let mut messages = vec![
        assistant_with_tool_calls("执行", &[("call-a", "ssh_exec"), ("call-b", "ssh_exec")]),
        tool_result("call-b", "ssh_exec", "结果B"),
        tool_result("call-a", "ssh_exec", "结果A"),
    ];

    repair_tool_call_pairing(&mut messages);

    assert_eq!(messages[1].tool_call_id.as_deref(), Some("call-a"));
    assert_eq!(messages[2].tool_call_id.as_deref(), Some("call-b"));
    assert!(messages[1].content.contains("结果A"));
}

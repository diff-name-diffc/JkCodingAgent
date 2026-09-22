//! 消息桥单元测试：`ChatMessage` → rig 消息的角色/内容映射、图片占位降级、
//! 引用提取与 attach 去重。
//!
//! 说明：上下文过滤（plumbing 工具结果、process-only assistant 消息）发生在
//! DB 侧历史加载（`load_llm_history` → `should_keep_llm_message`），
//! 其判定口径由 `agent::common::tests` 覆盖，本文件不重复。

use super::*;
use crate::agent::db::{FunctionCall, OutboundToolCall};

fn chat(role: &str, content: &str) -> ChatMessage {
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

async fn convert(messages: Vec<ChatMessage>) -> Vec<Message> {
    chat_history_to_rig(messages).await
}

#[tokio::test]
async fn system_and_user_messages_map_directly() {
    let messages = convert(vec![chat("system", "系统提示"), chat("user", "你好")]).await;
    assert_eq!(messages.len(), 2);
    assert!(matches!(&messages[0], Message::System { content } if content == "系统提示"));
    let Message::User { content } = &messages[1] else {
        panic!("user 消息应映射为 User");
    };
    assert_eq!(content.len(), 1);
    assert!(matches!(&content[0], UserContent::Text(t) if t.text == "你好"));
}

#[tokio::test]
async fn assistant_message_maps_reasoning_and_tool_calls() {
    let mut message = chat("assistant", "正文");
    message.reasoning_content = Some("思考链".to_string());
    message.tool_calls = Some(vec![OutboundToolCall {
        id: "call_1".to_string(),
        kind: "function".to_string(),
        function: FunctionCall {
            name: "read_file".to_string(),
            arguments: "{\"path\":\"a.rs\"}".to_string(),
        },
    }]);

    let messages = convert(vec![message]).await;
    let Some(Message::Assistant { content, .. }) = messages.first() else {
        panic!("assistant 消息应映射为 Assistant");
    };
    assert!(content
        .iter()
        .any(|item| matches!(item, AssistantContent::Reasoning(_))));
    assert!(content
        .iter()
        .any(|item| matches!(item, AssistantContent::Text(t) if t.text == "正文")));
    let call = content
        .iter()
        .find_map(|item| match item {
            AssistantContent::ToolCall(call) => Some(call),
            _ => None,
        })
        .expect("应带工具调用");
    assert_eq!(call.function.name, "read_file");
    assert_eq!(call.function.arguments["path"], "a.rs");
}

#[tokio::test]
async fn unparseable_tool_arguments_keep_raw_string() {
    let mut message = chat("assistant", "");
    message.tool_calls = Some(vec![OutboundToolCall {
        id: "call_1".to_string(),
        kind: "function".to_string(),
        function: FunctionCall {
            name: "read_file".to_string(),
            arguments: "not-json".to_string(),
        },
    }]);

    let messages = convert(vec![message]).await;
    let Some(Message::Assistant { content, .. }) = messages.first() else {
        panic!("assistant 消息应保留");
    };
    let call = content
        .iter()
        .find_map(|item| match item {
            AssistantContent::ToolCall(call) => Some(call),
            _ => None,
        })
        .expect("应带工具调用");
    assert_eq!(call.function.arguments, serde_json::json!("not-json"));
}

#[tokio::test]
async fn empty_assistant_message_is_dropped() {
    assert!(convert(vec![chat("assistant", "")]).await.is_empty());
}

#[tokio::test]
async fn tool_message_maps_to_tool_result() {
    let mut message = chat("tool", "工具输出");
    message.tool_call_id = Some("call_1".to_string());
    message.name = Some("read_file".to_string());

    let messages = convert(vec![message]).await;
    let Some(Message::User { content }) = messages.first() else {
        panic!("tool 消息应映射为 User 内的 ToolResult");
    };
    let UserContent::ToolResult(result) = &content[0] else {
        panic!("应包含 ToolResult");
    };
    assert_eq!(result.name, "read_file");
    assert!(matches!(
        &result.content[0],
        rig::message::ToolResultContent::Text(t) if t.text == "工具输出"
    ));
}

#[tokio::test]
async fn missing_chat_image_degrades_to_text_placeholder() {
    let mut message = chat("user", "看图");
    message.content_parts = vec![
        ChatMessageContentPart::Text {
            text: "看图".to_string(),
        },
        ChatMessageContentPart::Image {
            source: ChatMessageImageSource::ChatImage {
                image_id: "missing-image-id-1234".to_string(),
            },
        },
    ];

    let messages = convert(vec![message]).await;
    let Some(Message::User { content }) = messages.first() else {
        panic!("user 消息应保留");
    };
    assert!(content.iter().any(|item| match item {
        UserContent::Text(text) => text.text.contains("图片已丢失"),
        _ => false,
    }));
}

#[test]
fn extract_references_follows_whitelist_and_order() {
    let text = "先看 chat-image://AAAA1111BBBB2222 再看 chat-image://cccc3333dddd4444";
    let ids = extract_chat_image_references(text);
    assert_eq!(ids, vec!["AAAA1111BBBB2222", "cccc3333dddd4444"]);
    // 太短/含非法字符的引用不匹配。
    assert!(extract_chat_image_references("chat-image://short").is_empty());
    assert!(extract_chat_image_references("chat-image://bad/id").is_empty());
}

#[tokio::test]
async fn collect_turn_image_ids_dedupes_and_prefers_newest() {
    // 附加语义：只扫描「最后一条用户消息之后」的 assistant/tool 文本引用。
    let first_turn_user = Message::User {
        content: vec![UserContent::text("上一轮")],
    };
    let last_user = Message::User {
        content: vec![
            UserContent::text("任务"),
            // 已附加过的一张（引用文本再次出现时应去重）。
            UserContent::Image(chat_image(
                "base64data".to_string(),
                ImageMediaType::PNG,
                "already1234",
            )),
        ],
    };
    let assistant = Message::Assistant {
        id: None,
        content: vec![AssistantContent::text(
            "更早的 chat-image://old111112222 与最新的 chat-image://new333334444",
        )],
    };
    let messages = vec![first_turn_user, last_user, assistant.clone()];
    let last_user_index = 1;
    let ids = collect_turn_tool_image_ids(&messages, last_user_index);
    // already1234 已在用户消息中 → 去重；两个新引用按新→旧（消息逆序、引用逆序）。
    assert_eq!(ids, vec!["new333334444", "old111112222"]);

    // 引用全部位于最后一条用户消息之前 → 属历史轮次，不附加。
    let history_only = vec![
        Message::User {
            content: vec![UserContent::text("上一轮")],
        },
        assistant,
        Message::User {
            content: vec![UserContent::text("任务")],
        },
    ];
    assert!(collect_turn_tool_image_ids(&history_only, 2).is_empty());
}

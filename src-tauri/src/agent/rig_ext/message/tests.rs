//! 消息桥单元测试：角色映射、上下文过滤、引用提取与 attach 去重。

use super::*;
use crate::agent::db::content::ContentSegment;
use crate::agent::db::{FunctionCall, OutboundToolCall};

fn record(role: &str, content: &str) -> DispatcherMessageRecord {
    DispatcherMessageRecord {
        id: "m1".to_string(),
        workspace_id: "w1".to_string(),
        role: role.to_string(),
        segments_json: serde_json::to_string(&vec![ContentSegment::Text {
            id: "s1".to_string(),
            text: content.to_string(),
        }])
        .unwrap(),
        thinking_content: None,
        thinking_elapsed_ms: None,
        context_payload: None,
        tool_call_id: None,
        tool_name: None,
        tool_result_mode: None,
        tool_artifacts: Vec::new(),
        tool_calls_json: None,
        usage_stats: None,
        created_at: "2026-09-22T00:00:00Z".to_string(),
    }
}

fn tool_calls_json(calls: Vec<OutboundToolCall>) -> String {
    serde_json::to_string(&calls).unwrap()
}

#[tokio::test]
async fn system_and_user_records_map_directly() {
    let system = record_to_rig_message(&record("system", "系统提示")).await;
    assert!(matches!(system, Some(Message::System { content }) if content == "系统提示"));

    let user = record_to_rig_message(&record("user", "你好")).await;
    let Some(Message::User { content }) = user else {
        panic!("user 记录应映射为 User 消息");
    };
    assert_eq!(content.len(), 1);
    assert!(matches!(&content[0], UserContent::Text(t) if t.text == "你好"));
}

#[tokio::test]
async fn assistant_record_maps_reasoning_and_tool_calls() {
    let mut record = record("assistant", "正文");
    record.thinking_content = Some("思考链".to_string());
    record.tool_calls_json = Some(tool_calls_json(vec![OutboundToolCall {
        id: "call_1".to_string(),
        kind: "function".to_string(),
        function: FunctionCall {
            name: "read_file".to_string(),
            arguments: "{\"path\":\"a.rs\"}".to_string(),
        },
    }]));

    let Some(Message::Assistant { content, .. }) = record_to_rig_message(&record).await else {
        panic!("assistant 记录应映射为 Assistant 消息");
    };
    assert_eq!(content.len(), 3);
    assert!(
        matches!(&content[0], AssistantContent::Reasoning(r) if r.display_text() == "思考链")
    );
    assert!(matches!(&content[1], AssistantContent::Text(t) if t.text == "正文"));
    let AssistantContent::ToolCall(call) = &content[2] else {
        panic!("第三段应为 ToolCall");
    };
    assert_eq!(call.wire_call_id(), "call_1");
    assert_eq!(call.function.name, "read_file");
    assert_eq!(call.function.arguments["path"], "a.rs");
}

#[tokio::test]
async fn assistant_record_with_unparseable_arguments_keeps_raw_string() {
    let mut record = record("assistant", "");
    record.tool_calls_json = Some(tool_calls_json(vec![OutboundToolCall {
        id: "call_9".to_string(),
        kind: "function".to_string(),
        function: FunctionCall {
            name: "grep".to_string(),
            arguments: "not-json".to_string(),
        },
    }]));
    let Some(Message::Assistant { content, .. }) = record_to_rig_message(&record).await else {
        panic!("assistant 记录应映射为 Assistant 消息");
    };
    assert_eq!(content.len(), 1);
    let AssistantContent::ToolCall(call) = &content[0] else {
        panic!("应为 ToolCall");
    };
    assert_eq!(call.function.arguments, serde_json::Value::String("not-json".to_string()));
}

#[tokio::test]
async fn empty_assistant_record_is_dropped() {
    assert!(record_to_rig_message(&record("assistant", "")).await.is_none());
}

#[tokio::test]
async fn tool_record_maps_to_tool_result() {
    let mut record = record("tool", "展示文本");
    record.context_payload = Some("回灌负载".to_string());
    record.tool_call_id = Some("call_1".to_string());
    record.tool_name = Some("read_file".to_string());

    let Some(Message::User { content }) = record_to_rig_message(&record).await else {
        panic!("tool 记录应映射为 User(ToolResult) 消息");
    };
    let UserContent::ToolResult(result) = &content[0] else {
        panic!("应为 ToolResult");
    };
    assert_eq!(result.wire_call_id(), "call_1");
    assert_eq!(result.name, "read_file");
    assert_eq!(result.content[0].as_text(), Some("回灌负载"));
}

#[tokio::test]
async fn plumbing_and_process_only_records_are_filtered() {
    // 纯调度 plumbing 工具结果不进上下文（与 should_keep_llm_message 同口径）。
    let mut plumbing = record("tool", "dispatch 结果");
    plumbing.tool_name = Some("dispatch_claude".to_string());
    plumbing.tool_call_id = Some("c1".to_string());
    assert!(record_to_rig_message(&plumbing).await.is_none());

    // process-only assistant 消息不进上下文。
    assert!(record_to_rig_message(&record("assistant", "📋 已提交 执行图"))
        .await
        .is_none());

    // 普通消息不受影响。
    assert!(record_to_rig_message(&record("assistant", "正常回复"))
        .await
        .is_some());
}

#[tokio::test]
async fn missing_chat_image_degrades_to_text_placeholder() {
    let segments = vec![
        ContentSegment::Image {
            id: "s1".to_string(),
            image_id: "missing-image-id".to_string(),
            alt: None,
            width: None,
            height: None,
            mime_type: None,
            source: "paste".to_string(),
            generation_prompt: None,
        },
        ContentSegment::Text {
            id: "s2".to_string(),
            text: "看图".to_string(),
        },
    ];
    let mut record = record("user", "");
    record.segments_json = serde_json::to_string(&segments).unwrap();

    let Some(Message::User { content }) = record_to_rig_message(&record).await else {
        panic!("user 记录应映射为 User 消息");
    };
    let placeholder = content.iter().any(|item| {
        matches!(item, UserContent::Text(t) if t.text.contains("[图片已丢失：chat-image://missing-image-id"))
    });
    assert!(placeholder, "缺失图片应降级为占位文本：{content:?}");
}

#[test]
fn extract_references_follows_whitelist_and_order() {
    let text = "看 chat-image://abc-def01 和 chat-image://xyz12345，再排除 chat-image://短 与 chat-image://";
    assert_eq!(
        extract_chat_image_references(text),
        vec!["abc-def01".to_string(), "xyz12345".to_string()]
    );
}

#[test]
fn collect_turn_image_ids_dedupes_and_prefers_newest() {
    let user = Message::User {
        content: vec![UserContent::Image(Image {
            data: DocumentSourceKind::Base64("a".to_string()),
            media_type: Some(ImageMediaType::PNG),
            detail: None,
            additional_params: AdditionalParams::from_entries(Some((
                CHAT_IMAGE_ID_PARAM,
                serde_json::Value::String("attached1".to_string()),
            ))),
        })],
    };
    let assistant = Message::Assistant {
        id: None,
        content: vec![AssistantContent::Text(Text::new(
            "先看 chat-image://newone01，再看 chat-image://attached1",
        ))],
    };
    let messages = vec![user, assistant];
    let ids = collect_turn_tool_image_ids(&messages, 0);
    assert_eq!(ids, vec!["newone01".to_string()]);
}

#[test]
fn collect_turn_image_ids_truncates_to_limit() {
    let user = Message::User {
        content: vec![UserContent::text("开始")],
    };
    let mut texts = Vec::new();
    for index in 0..6 {
        texts.push(format!("chat-image://image-id-{index:04}"));
    }
    let assistant = Message::Assistant {
        id: None,
        content: vec![AssistantContent::Text(Text::new(texts.join(" ")))],
    };
    let messages = vec![user, assistant];
    let ids = collect_turn_tool_image_ids(&messages, 0);
    // 越新的引用优先：逆序取前 3 个。
    assert_eq!(ids.len(), MAX_TURN_TOOL_IMAGE_ATTACHMENTS);
    assert_eq!(ids[0], "image-id-0005");
}

#[test]
fn data_url_image_requires_image_mime() {
    assert!(data_url_to_image("data:text/plain;base64,aGk=").is_err());
    let image = data_url_to_image("data:image/png;base64,aGk=").unwrap();
    assert_eq!(image.media_type, Some(ImageMediaType::PNG));
    assert_eq!(
        image.data,
        DocumentSourceKind::Base64("aGk=".to_string())
    );
}

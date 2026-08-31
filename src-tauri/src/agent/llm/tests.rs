use reqwest::StatusCode;

use super::protocol::{parse_sse_data_line, should_retry_without_extra_fields, StreamChatRequest};
use super::request::{build_api_message_content, ApiMessageContent, ApiMessageContentPart};
use super::*;

#[test]
fn sse_data_line_accepts_missing_space_and_rejects_non_data_lines() {
    assert_eq!(parse_sse_data_line("data: {\"a\":1}"), Some("{\"a\":1}"));
    assert_eq!(parse_sse_data_line("data:{\"a\":1}"), Some("{\"a\":1}"));
    assert_eq!(parse_sse_data_line("data:[DONE]"), Some("[DONE]"));
    assert_eq!(parse_sse_data_line("data:   "), None);
    assert_eq!(parse_sse_data_line("data:"), None);
    assert_eq!(parse_sse_data_line("event: message"), None);
    assert_eq!(parse_sse_data_line(""), None);
}

#[test]
fn stream_chat_request_skips_none_fields_and_keeps_some_fields() {
    let plain = StreamChatRequest {
        model: "test-model",
        messages: &[],
        max_tokens: Some(16),
        temperature: 0.0,
        stream: true,
        enable_thinking: None,
        stream_options: None,
        tools: None,
    };
    let value = serde_json::to_value(&plain).expect("request serializable");
    assert!(value.get("enable_thinking").is_none());
    assert!(value.get("stream_options").is_none());
    assert!(value.get("tools").is_none());
    assert_eq!(value["max_tokens"], serde_json::json!(16));

    // without_max_tokens 路径：max_tokens=None 时请求体完全省略该字段
    let unbounded = StreamChatRequest {
        model: "test-model",
        messages: &[],
        max_tokens: None,
        temperature: 0.0,
        stream: true,
        enable_thinking: None,
        stream_options: None,
        tools: None,
    };
    let value = serde_json::to_value(&unbounded).expect("request serializable");
    assert!(value.get("max_tokens").is_none());

    let with_extras = StreamChatRequest {
        model: "test-model",
        messages: &[],
        max_tokens: Some(16),
        temperature: 0.0,
        stream: true,
        enable_thinking: Some(false),
        stream_options: Some(StreamOptions {
            include_usage: true,
        }),
        tools: None,
    };
    let value = serde_json::to_value(&with_extras).expect("request serializable");
    assert_eq!(value["enable_thinking"], serde_json::json!(false));
    assert_eq!(
        value["stream_options"]["include_usage"],
        serde_json::json!(true)
    );
}

#[test]
fn retry_heuristic_covers_enable_thinking_and_stream_options_rejections() {
    let bad_request = StatusCode::BAD_REQUEST;
    assert!(should_retry_without_extra_fields(
        bad_request,
        "Unsupported parameter: 'enable_thinking' is not supported with this model.",
        false,
        true
    ));
    assert!(should_retry_without_extra_fields(
        bad_request,
        "Unknown parameter: 'stream_options'",
        true,
        false
    ));
    assert!(should_retry_without_extra_fields(
        bad_request,
        "Required body invalid",
        true,
        true
    ));
    // 没有可移除字段时不重试
    assert!(!should_retry_without_extra_fields(
        bad_request,
        "Unsupported parameter: 'enable_thinking'",
        false,
        false
    ));
    // 非 400 不重试
    assert!(!should_retry_without_extra_fields(
        StatusCode::UNAUTHORIZED,
        "enable_thinking",
        true,
        true
    ));
    // 与其他字段相关的 400 不触发重试
    assert!(!should_retry_without_extra_fields(
        bad_request,
        "messages is required",
        true,
        true
    ));
}

fn data_url_image_message(content: &str, parts: Vec<ChatMessageContentPart>) -> ChatMessage {
    ChatMessage {
        role: "user".to_string(),
        content: content.to_string(),
        content_parts: parts,
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
    }
}

fn data_url_part() -> ChatMessageContentPart {
    ChatMessageContentPart::Image {
        source: ChatMessageImageSource::DataUrl {
            data_url: "data:image/png;base64,AAA=".to_string(),
        },
    }
}

#[tokio::test]
async fn build_api_message_content_restores_text_from_content_when_missing() {
    let message = data_url_image_message("请看这张图片", vec![data_url_part()]);

    let content = build_api_message_content(&message)
        .await
        .expect("multimodal content built");
    let ApiMessageContent::Parts(parts) = content else {
        panic!("expected parts for message with image");
    };

    assert_eq!(parts.len(), 2);
    match &parts[0] {
        ApiMessageContentPart::Text { text } => assert_eq!(text, "请看这张图片"),
        other => panic!("expected text part first, got {other:?}"),
    }
    assert!(matches!(parts[1], ApiMessageContentPart::ImageUrl { .. }));
}

#[tokio::test]
async fn build_api_message_content_keeps_existing_text_parts() {
    let message = data_url_image_message(
        "不应重复注入的文本",
        vec![
            ChatMessageContentPart::Text {
                text: "已有文本".to_string(),
            },
            data_url_part(),
        ],
    );

    let content = build_api_message_content(&message)
        .await
        .expect("multimodal content built");
    let ApiMessageContent::Parts(parts) = content else {
        panic!("expected parts for message with image");
    };

    assert_eq!(parts.len(), 2);
    match &parts[0] {
        ApiMessageContentPart::Text { text } => assert_eq!(text, "已有文本"),
        other => panic!("expected existing text part first, got {other:?}"),
    }
}

#[tokio::test]
async fn build_api_message_content_degrades_missing_chat_image_to_placeholder() {
    // 引用不存在的 image_id：不得让请求失败，也不得丢掉用户文本，
    // 而是以「图片已丢失」占位文本继续。
    let message = data_url_image_message(
        "这张图里是什么",
        vec![ChatMessageContentPart::Image {
            source: ChatMessageImageSource::ChatImage {
                image_id: "00000000-dead-4000-8000-000000000000".to_string(),
            },
        }],
    );

    let content = build_api_message_content(&message)
        .await
        .expect("missing image must not fail the request");
    let ApiMessageContent::Parts(parts) = content else {
        panic!("expected placeholder parts even when every image is missing");
    };

    assert!(parts.iter().any(|part| match part {
        ApiMessageContentPart::Text { text } => {
            text.starts_with("[图片已丢失：chat-image://") && text.ends_with("，已跳过]")
        }
        _ => false,
    }));
    assert!(!parts
        .iter()
        .any(|part| matches!(part, ApiMessageContentPart::ImageUrl { .. })));
    match &parts[0] {
        ApiMessageContentPart::Text { text } => assert_eq!(text, "这张图里是什么"),
        other => panic!("expected user text preserved first, got {other:?}"),
    }
}

#[tokio::test]
async fn build_api_message_content_degrades_malformed_data_url() {
    let message = data_url_image_message(
        "",
        vec![ChatMessageContentPart::Image {
            source: ChatMessageImageSource::DataUrl {
                data_url: "http://example.com/not-a-data-url".to_string(),
            },
        }],
    );

    let content = build_api_message_content(&message)
        .await
        .expect("malformed data url must not fail the request");
    let ApiMessageContent::Parts(parts) = content else {
        panic!("expected placeholder parts for malformed data url");
    };

    assert!(parts.iter().any(|part| matches!(
        part,
        ApiMessageContentPart::Text { text } if text.contains("[图片已丢失：")
    )));
    assert!(!parts
        .iter()
        .any(|part| matches!(part, ApiMessageContentPart::ImageUrl { .. })));
}

fn tool_message(content: &str) -> ChatMessage {
    ChatMessage {
        role: "tool".to_string(),
        content: content.to_string(),
        content_parts: Vec::new(),
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: Some("call-1".to_string()),
        name: Some("mcp_camera".to_string()),
    }
}

fn chat_image_part_ids(message: &ChatMessage) -> Vec<String> {
    message
        .content_parts
        .iter()
        .filter_map(|part| match part {
            ChatMessageContentPart::Image {
                source: ChatMessageImageSource::ChatImage { image_id },
            } => Some(image_id.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn attach_turn_tool_images_appends_current_turn_references_to_last_user_message() {
    let messages = vec![
        ChatMessage::system("system".to_string()),
        data_url_image_message("上一轮", vec![]),
        tool_message("上一轮的旧图 ![旧图](chat-image://aaaaaaaa-0000-4000-8000-000000000001)"),
        data_url_image_message("这轮看看 MCP 拍的图", vec![]),
        tool_message("抓拍成功：![快照](chat-image://bbbbbbbb-0000-4000-8000-000000000002)"),
    ];

    let attached = attach_turn_tool_images(&messages);

    // 只附加最后一条用户消息之后出现的引用；旧轮次的引用不动。
    let last_user = attached
        .iter()
        .rposition(|message| message.role == "user")
        .map(|index| &attached[index])
        .unwrap();
    let ids = chat_image_part_ids(last_user);
    assert_eq!(
        ids,
        vec!["bbbbbbbb-0000-4000-8000-000000000002".to_string()]
    );
    // 附加后多模态开关命中（vision 槽位自动切换的判定输入）。
    assert!(messages_contain_images(&attached));
    // 原列表不被修改（每次请求重算，不污染持久历史）。
    let original_user = &messages[3];
    assert_eq!(original_user.role, "user");
    assert_eq!(chat_image_part_ids(original_user), Vec::<String>::new());
}

#[test]
fn attach_turn_tool_images_is_idempotent_across_iterations() {
    let messages = vec![
        data_url_image_message("这轮", vec![]),
        tool_message("生成成功 ![图](chat-image://cccccccc-0000-4000-8000-000000000003)"),
    ];

    // 第一次附加 → 再次对（模拟下一次请求重建的）带 parts 列表重算 → 不再重复。
    let once = attach_turn_tool_images(&messages);
    let twice = attach_turn_tool_images(&once);
    assert_eq!(
        chat_image_part_ids(&twice[0]),
        vec!["cccccccc-0000-4000-8000-000000000003".to_string()]
    );
}

#[test]
fn attach_turn_tool_images_caps_to_most_recent_references() {
    let mut messages = vec![data_url_image_message("这轮", vec![])];
    for index in 0..5 {
        let id = format!("dddddddd-0000-4000-8000-{index:012}");
        messages.push(tool_message(&format!(
            "第 {index} 张 ![图](chat-image://{id})"
        )));
    }

    let attached = attach_turn_tool_images(&messages);
    let ids = chat_image_part_ids(attached.first().unwrap());
    assert_eq!(ids.len(), MAX_TURN_TOOL_IMAGE_ATTACHMENTS);
    // 越靠后（越新）的引用优先：保留 index 4、3、2。
    assert_eq!(
        ids,
        [
            "dddddddd-0000-4000-8000-000000000004",
            "dddddddd-0000-4000-8000-000000000003",
            "dddddddd-0000-4000-8000-000000000002",
        ]
        .map(str::to_string)
        .to_vec()
    );
}

#[test]
fn attach_turn_tool_images_ignores_duplicated_and_malformed_references() {
    let messages = vec![
        data_url_image_message("这轮", vec![]),
        tool_message(
            "同一张图被模型复述：![a](chat-image://eeeeeeee-0000-4000-8000-000000000005) \
             和 ![b](chat-image://eeeeeeee-0000-4000-8000-000000000005)，\
             编造的 ![c](chat-image://short) 与 ![d](chat-image://含中文id) 不算",
        ),
    ];

    let attached = attach_turn_tool_images(&messages);
    assert_eq!(
        chat_image_part_ids(&attached[0]),
        vec!["eeeeeeee-0000-4000-8000-000000000005".to_string()]
    );
}

#[test]
fn attach_turn_tool_images_without_user_message_is_noop() {
    let messages = vec![ChatMessage::system("system".to_string())];
    let attached = attach_turn_tool_images(&messages);
    assert_eq!(attached.len(), 1);
    assert!(attached[0].content_parts.is_empty());
}

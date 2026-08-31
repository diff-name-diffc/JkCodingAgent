use std::path::Path;

use anyhow::{Context, Result};
use base64::Engine;
use serde::Serialize;

use super::{ChatMessage, ChatMessageContentPart, ChatMessageImageSource, OutboundToolCall};

#[derive(Debug, Clone, Serialize)]
pub(super) struct ApiChatMessage {
    role: String,
    content: ApiMessageContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OutboundToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub(super) enum ApiMessageContent {
    Text(String),
    Parts(Vec<ApiMessageContentPart>),
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ApiMessageContentPart {
    Text { text: String },
    ImageUrl { image_url: ApiImageUrl },
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ApiImageUrl {
    url: String,
}

pub(super) async fn build_api_messages(
    messages: &[ChatMessage],
    enable_multimodal: bool,
) -> Result<Vec<ApiChatMessage>> {
    let mut api_messages = Vec::with_capacity(messages.len());

    for message in messages {
        api_messages.push(ApiChatMessage {
            role: message.role.clone(),
            content: if enable_multimodal && message.role == "user" && message_has_image(message) {
                build_api_message_content(message).await?
            } else {
                ApiMessageContent::Text(message.content.clone())
            },
            reasoning_content: message
                .reasoning_content
                .as_ref()
                .filter(|content| message.role == "assistant" && !content.trim().is_empty())
                .cloned(),
            tool_calls: message.tool_calls.clone(),
            tool_call_id: message.tool_call_id.clone(),
            name: message.name.clone(),
        });
    }

    Ok(api_messages)
}

pub(super) async fn build_api_message_content(message: &ChatMessage) -> Result<ApiMessageContent> {
    let mut parts = Vec::new();
    let mut has_live_image = false;
    let mut has_missing_image = false;
    let mut has_source_text = false;

    for part in &message.content_parts {
        match part {
            ChatMessageContentPart::Text { text } => {
                if !text.trim().is_empty() {
                    has_source_text = true;
                }
                push_text_part(&mut parts, text);
            }
            ChatMessageContentPart::Image { source } => match image_source_for_api(source).await {
                ImageApiPart::DataUrl(url) => {
                    has_live_image = true;
                    parts.push(ApiMessageContentPart::ImageUrl {
                        image_url: ApiImageUrl { url },
                    });
                }
                ImageApiPart::Missing(reason) => {
                    // 图片丢失只降级为文本占位，绝不打断整个 run（标题生成、
                    // 摘要、主循环共用此路径）。占位不算「已有文本」，
                    // 否则会挤掉下方补入 content 的用户指令。
                    has_missing_image = true;
                    push_text_part(&mut parts, &format!("[图片已丢失：{reason}，已跳过]"));
                }
            },
        }
    }

    if has_live_image || has_missing_image {
        // 说明文字可能只存在于 content（如 context_payload 恢复的消息或调用方
        // 只构造了图片 part）：没有可读文本 part 时把 content 补为首个 Text part，
        // 避免用户指令被静默丢弃。
        if !has_source_text && !message.content.trim().is_empty() {
            parts.insert(
                0,
                ApiMessageContentPart::Text {
                    text: message.content.trim().to_string(),
                },
            );
        }
        Ok(ApiMessageContent::Parts(parts))
    } else {
        Ok(ApiMessageContent::Text(message.content.clone()))
    }
}

fn message_has_image(message: &ChatMessage) -> bool {
    message
        .content_parts
        .iter()
        .any(|part| matches!(part, ChatMessageContentPart::Image { .. }))
}

fn push_text_part(parts: &mut Vec<ApiMessageContentPart>, text: &str) {
    let trimmed = text.trim();
    if !trimmed.is_empty() {
        parts.push(ApiMessageContentPart::Text {
            text: trimmed.to_string(),
        });
    }
}

/// 单个图片 part 的 API 形态：可发送的 data URL，或不可发送（文件缺失/
/// 读取失败/超限）时的降级说明。降级信息会以文本占位进入请求，模型可见
/// 图片丢失的事实，而不是让整个请求失败。
#[derive(Debug, Clone)]
enum ImageApiPart {
    DataUrl(String),
    Missing(String),
}

async fn image_source_for_api(source: &ChatMessageImageSource) -> ImageApiPart {
    match source {
        ChatMessageImageSource::DataUrl { data_url } => {
            if data_url.starts_with("data:image/") {
                ImageApiPart::DataUrl(data_url.clone())
            } else {
                ImageApiPart::Missing("data URL 必须以 data:image/ 开头".to_string())
            }
        }
        ChatMessageImageSource::ChatImage { image_id } => {
            let reference = format!("chat-image://{image_id}");
            let resolved =
                match crate::chat_images::resolve_chat_image_id_async(image_id.clone()).await {
                    Ok(path) => path,
                    Err(error) => return ImageApiPart::Missing(format!("{reference}（{error}）")),
                };
            match tokio::task::spawn_blocking(move || local_image_to_data_url(&resolved)).await {
                Ok(Ok(url)) => ImageApiPart::DataUrl(url),
                Ok(Err(error)) => ImageApiPart::Missing(format!("{reference}（{error:#}）")),
                Err(error) => {
                    ImageApiPart::Missing(format!("{reference}（后台读取任务失败：{error}）"))
                }
            }
        }
    }
}

const MAX_INLINE_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

fn local_image_to_data_url(path: &Path) -> Result<String> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("读取图片元数据失败：{}", path.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("图片路径不是文件：{}", path.display());
    }
    if metadata.len() > MAX_INLINE_IMAGE_BYTES {
        anyhow::bail!(
            "图片文件过大：{}，当前限制为 {} MB",
            path.display(),
            MAX_INLINE_IMAGE_BYTES / 1024 / 1024
        );
    }

    let mime = image_mime_from_path(path)?;
    let bytes = std::fs::read(path).with_context(|| format!("读取图片失败：{}", path.display()))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:{mime};base64,{encoded}"))
}

fn image_mime_from_path(path: &Path) -> Result<&'static str> {
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    crate::chat_images::mime_for_ext(ext)
        .ok_or_else(|| anyhow::anyhow!("不支持的图片格式：{}", path.display()))
}

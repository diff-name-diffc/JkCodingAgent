use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::{json, Value};

use crate::agent::llm::{ChatMessage, OpenAiCompatProvider};

use super::document;
use super::types::{KnowledgeModelConfig, KnowledgeSettings};
use super::utils::{spawn_blocking_string, truncate_chars};

pub(crate) async fn call_text_model(
    model: &KnowledgeModelConfig,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<String, String> {
    let base = normalize_chat_base_url(&model.url);
    let provider =
        OpenAiCompatProvider::new(model.api_key.clone(), base, model.model.clone(), 8192, 0.1);
    let messages = vec![
        ChatMessage::system(system_prompt.to_string()),
        ChatMessage {
            role: "user".to_string(),
            content: user_prompt.to_string(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        },
    ];
    let mut _sink = String::new();
    provider
        .chat_stream(&messages, &[], true, |delta| _sink.push_str(delta))
        .await
        .map(|response| response.content)
        .map_err(|error| error.to_string())
}

pub(crate) async fn fetch_embedding(
    text: &str,
    model: &KnowledgeModelConfig,
) -> Result<Vec<f32>, String> {
    let endpoint = normalize_embedding_endpoint(&model.url);
    let client = reqwest::Client::new();
    let mut input = text.trim().to_string();
    if input.is_empty() {
        input = " ".to_string();
    }
    for _ in 0..5 {
        let mut request = client.post(&endpoint).json(&json!({
            "model": model.model,
            "input": input,
        }));
        if !model.api_key.trim().is_empty() {
            request = request.bearer_auth(model.api_key.trim());
        }
        let response = request.send().await.map_err(|error| error.to_string())?;
        let status = response.status();
        let body = response.text().await.map_err(|error| error.to_string())?;
        if status.is_success() {
            let value: Value = serde_json::from_str(&body).map_err(|error| error.to_string())?;
            return parse_embedding_response(&value)
                .ok_or_else(|| "embedding 响应中未找到 data[0].embedding".to_string());
        }
        let lower = body.to_lowercase();
        if input.len() > 200
            && (lower.contains("too long")
                || lower.contains("maximum context")
                || lower.contains("tokens"))
        {
            input.truncate(input.len() / 2);
            continue;
        }
        return Err(format!("embedding 请求失败，HTTP {status}：{body}"));
    }
    Err("embedding 文本过长，自动减半重试后仍失败。".to_string())
}

pub(crate) fn parse_embedding_response(value: &Value) -> Option<Vec<f32>> {
    value
        .get("data")?
        .as_array()?
        .first()?
        .get("embedding")?
        .as_array()?
        .iter()
        .map(|item| item.as_f64().map(|v| v as f32))
        .collect()
}

fn normalize_chat_base_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    trimmed
        .strip_suffix("/chat/completions")
        .or_else(|| trimmed.strip_suffix("/v1/chat/completions"))
        .unwrap_or(trimmed)
        .to_string()
}

fn normalize_embedding_endpoint(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    if trimmed.ends_with("/embeddings") {
        trimmed.to_string()
    } else if trimmed.ends_with("/v1") {
        format!("{trimmed}/embeddings")
    } else {
        format!("{trimmed}/v1/embeddings")
    }
}

pub(crate) fn configured_vision_model(
    settings: &KnowledgeSettings,
) -> Option<KnowledgeModelConfig> {
    let model = &settings.vision_model;
    if model.url.trim().is_empty() || model.model.trim().is_empty() {
        None
    } else {
        Some(model.clone())
    }
}

pub(crate) async fn image_data_url(image: &document::SavedImage) -> Result<String, String> {
    let image = image.clone();
    spawn_blocking_string(move || {
        let bytes = std::fs::read(&image.abs_path)
            .with_context(|| format!("读取知识库图片失败：{}", image.abs_path))?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(anyhow::anyhow!(
                "图片超过 8MB，拒绝发送给多模态模型：{}",
                image.abs_path
            ));
        }
        Ok(format!(
            "data:{};base64,{}",
            image.mime_type,
            BASE64.encode(bytes)
        ))
    })
    .await
}

const MAX_SOURCE_CHARS: usize = 120_000;
const MAX_IMAGE_CAPTIONS_PER_SOURCE: usize = 50;

pub(crate) async fn enrich_source_text_with_image_captions(
    extraction: &document::DocumentExtraction,
    settings: &KnowledgeSettings,
) -> Result<String, String> {
    let mut source_text = truncate_chars(&extraction.markdown, MAX_SOURCE_CHARS);
    if extraction.images.is_empty() {
        return Ok(source_text);
    }

    let Some(model) = configured_vision_model(settings) else {
        return Ok(source_text);
    };

    let mut caption_blocks = Vec::new();
    for image in extraction.images.iter().take(MAX_IMAGE_CAPTIONS_PER_SOURCE) {
        let data_url = image_data_url(image).await?;
        let prompt = format!(
            "请为这张知识库导入图片生成简体中文 caption。\n\
要求：\n\
- 一句话概括图片内容。\n\
- 如果是图表、表格、截图，提取关键文字、数值、实体和关系。\n\
- 不要杜撰看不见的信息。\n\n\
图片路径：{}\n图片：![image]({})",
            image.rel_path, data_url
        );
        let caption = call_text_model(
            &model,
            "你是知识库图片标注助手，只输出可检索的图片说明。",
            &prompt,
        )
        .await?;
        caption_blocks.push(format!(
            "![Image {}]({})\n\nCaption: {}",
            image.index,
            image.rel_path,
            caption.trim()
        ));
    }

    if !caption_blocks.is_empty() {
        source_text.push_str("\n\n## Image Captions\n\n");
        source_text.push_str(&caption_blocks.join("\n\n"));
        source_text.push('\n');
    }
    Ok(truncate_chars(&source_text, MAX_SOURCE_CHARS))
}

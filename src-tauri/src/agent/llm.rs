use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use futures::StreamExt;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::shared::truncate_for_display;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OutboundToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    pub fn system(content: String) -> Self {
        Self {
            role: "system".to_string(),
            content,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct ApiChatMessage {
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
enum ApiMessageContent {
    Text(String),
    Parts(Vec<ApiMessageContentPart>),
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ApiMessageContentPart {
    Text { text: String },
    ImageUrl { image_url: ApiImageUrl },
}

#[derive(Debug, Clone, Serialize)]
struct ApiImageUrl {
    url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolFunctionDefinition,
}

#[derive(Debug, Clone, Serialize)]
pub struct RequestedToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmResponse {
    pub status_code: u16,
    pub content: String,
    pub thinking_content: String,
    pub thinking_elapsed_ms: u64,
    pub tool_calls: Vec<RequestedToolCall>,
    pub raw_response: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<LlmUsage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmRequestSnapshot {
    pub method: String,
    pub url: String,
    pub headers: LlmRequestHeadersSnapshot,
    pub body: LlmRequestBodySnapshot,
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmRequestHeadersSnapshot {
    pub authorization: String,
    pub content_type: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmRequestBodySnapshot {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub max_tokens: u32,
    pub temperature: f32,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmResponseSnapshot {
    pub status_code: u16,
    pub body: LlmResponseBodySnapshot,
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmResponseBodySnapshot {
    pub model: String,
    pub content: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub thinking_content: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub thinking_elapsed_ms: u64,
    pub tool_calls: Vec<RequestedToolCall>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub raw_response: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<LlmUsage>,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmPromptTokensDetails {
    #[serde(default)]
    pub cached_tokens: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub prompt_tokens_details: Option<LlmPromptTokensDetails>,
}

impl LlmUsage {
    pub fn cached_tokens(&self) -> u64 {
        self.prompt_tokens_details
            .as_ref()
            .map_or(0, |details| details.cached_tokens)
    }
}

pub fn messages_contain_inline_images(messages: &[ChatMessage]) -> bool {
    messages
        .iter()
        .any(|message| message.role == "user" && content_contains_inline_image(&message.content))
}

fn append_valid_utf8(buffer: &mut String, leftover_bytes: &mut Vec<u8>, bytes: &[u8]) {
    leftover_bytes.extend_from_slice(bytes);

    loop {
        match std::str::from_utf8(leftover_bytes) {
            Ok(text) => {
                buffer.push_str(text);
                leftover_bytes.clear();
                break;
            }
            Err(error) => {
                let valid_len = error.valid_up_to();
                if valid_len > 0 {
                    buffer.push_str(
                        std::str::from_utf8(&leftover_bytes[..valid_len])
                            .expect("valid UTF-8 prefix"),
                    );
                    leftover_bytes.drain(..valid_len);
                    continue;
                }

                if let Some(invalid_len) = error.error_len() {
                    leftover_bytes.drain(..invalid_len);
                    continue;
                }

                // A valid scalar can be at most 4 bytes. Anything older than that
                // cannot be a legitimate incomplete UTF-8 tail.
                if leftover_bytes.len() > 4 {
                    let drop_len = leftover_bytes.len() - 4;
                    leftover_bytes.drain(..drop_len);
                    continue;
                }

                break;
            }
        }
    }
}

#[derive(Clone)]
pub struct OpenAiCompatProvider {
    client: Client,
    api_key: String,
    api_base: String,
    model: String,
    max_tokens: u32,
    temperature: f32,
}

impl OpenAiCompatProvider {
    pub fn new(
        api_key: String,
        api_base: String,
        model: String,
        max_tokens: u32,
        temperature: f32,
    ) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .expect("build http client"),
            api_key,
            api_base,
            model,
            max_tokens,
            temperature,
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.api_key.trim().is_empty()
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    pub fn api_base(&self) -> &str {
        &self.api_base
    }

    pub fn with_model(&self, model: impl Into<String>) -> Self {
        let mut next = self.clone();
        next.model = model.into();
        next
    }

    pub fn build_request_snapshot(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        enable_thinking: bool,
    ) -> LlmRequestSnapshot {
        LlmRequestSnapshot {
            method: "POST".to_string(),
            url: format!("{}/chat/completions", self.api_base.trim_end_matches('/')),
            headers: LlmRequestHeadersSnapshot {
                authorization: "Bearer ***".to_string(),
                content_type: "application/json".to_string(),
            },
            body: LlmRequestBodySnapshot {
                model: self.model.clone(),
                messages: messages.to_vec(),
                max_tokens: self.max_tokens,
                temperature: self.temperature,
                stream: true,
                enable_thinking: self.enable_thinking_parameter(enable_thinking),
                stream_options: Some(StreamOptions {
                    include_usage: true,
                }),
                tools: if tools.is_empty() {
                    None
                } else {
                    Some(tools.to_vec())
                },
            },
        }
    }

    pub fn build_response_snapshot(&self, response: &LlmResponse) -> LlmResponseSnapshot {
        LlmResponseSnapshot {
            status_code: response.status_code,
            body: LlmResponseBodySnapshot {
                model: self.model.clone(),
                content: response.content.clone(),
                thinking_content: response.thinking_content.clone(),
                thinking_elapsed_ms: response.thinking_elapsed_ms,
                tool_calls: response.tool_calls.clone(),
                raw_response: response.raw_response.clone(),
                usage: response.usage.clone(),
            },
        }
    }

    /// Streaming chat completion. Calls `on_delta` for each content token.
    pub async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        enable_multimodal: bool,
        on_delta: impl FnMut(&str),
    ) -> Result<LlmResponse> {
        self.chat_stream_with_thinking(
            messages,
            tools,
            enable_multimodal,
            false,
            on_delta,
            |_, _| {},
        )
        .await
    }

    /// Streaming chat completion with optional model-side thinking enabled.
    pub async fn chat_stream_with_thinking(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        enable_multimodal: bool,
        enable_thinking: bool,
        mut on_delta: impl FnMut(&str),
        mut on_thinking_delta: impl FnMut(&str, u64),
    ) -> Result<LlmResponse> {
        if !self.is_configured() {
            return Err(anyhow!("LLM API Key 尚未配置。"));
        }

        let url = format!("{}/chat/completions", self.api_base.trim_end_matches('/'));
        let api_messages = build_api_messages(messages, enable_multimodal).await?;
        let mut request = StreamChatRequest {
            model: &self.model,
            messages: &api_messages,
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            tools: if tools.is_empty() { None } else { Some(tools) },
            stream: true,
            enable_thinking: self.enable_thinking_parameter(enable_thinking),
            stream_options: Some(StreamOptions {
                include_usage: true,
            }),
        };

        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await
            .context("发送流式对话请求失败")?;

        let mut status = response.status();
        let response = if status.is_success() {
            response
        } else {
            let body = response.text().await.context("读取 LLM 错误响应失败")?;
            if should_retry_without_stream_options(status, &body, request.stream_options.is_some())
            {
                request.stream_options = None;
                let retry_response = self
                    .client
                    .post(&url)
                    .bearer_auth(&self.api_key)
                    .json(&request)
                    .send()
                    .await
                    .context("发送无 stream_options 的流式对话重试请求失败")?;
                status = retry_response.status();
                if status.is_success() {
                    retry_response
                } else {
                    let retry_body = retry_response
                        .text()
                        .await
                        .context("读取 LLM 重试错误响应失败")?;
                    return Err(anyhow!(
                        "LLM 请求失败，HTTP {}：{}；去除 stream_options 后仍失败，HTTP {}：{}",
                        StatusCode::BAD_REQUEST,
                        body,
                        status,
                        retry_body
                    ));
                }
            } else {
                return Err(anyhow!("LLM 请求失败，HTTP {}：{}", status, body));
            }
        };

        if !status.is_success() {
            let body = response.text().await.context("读取 LLM 错误响应失败")?;
            return Err(anyhow!("LLM 请求失败，HTTP {}：{}", status, body));
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut leftover_bytes: Vec<u8> = Vec::new();
        let mut content = String::new();
        let mut thinking_content = String::new();
        let mut thinking_started_at: Option<std::time::Instant> = None;
        let mut thinking_elapsed_ms = 0_u64;
        let mut raw_response = String::new();
        let mut usage: Option<LlmUsage> = None;
        // index -> (id, name, accumulated_arguments)
        let mut tc_map: BTreeMap<usize, (String, String, String)> = BTreeMap::new();

        while let Some(chunk) = stream.next().await {
            let bytes = chunk.context("读取流式响应分片失败")?;
            if buffer.len() > 1_000_000 {
                return Err(anyhow!("SSE 行超过最大缓冲区大小"));
            }
            append_valid_utf8(&mut buffer, &mut leftover_bytes, &bytes);

            // Process complete lines from the buffer
            while let Some(newline_pos) = buffer.find('\n') {
                let line_end = buffer[..newline_pos].trim_end().len();
                let line = buffer[..line_end].to_string();
                buffer.drain(..newline_pos + 1);

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }
                let data = if let Some(stripped) = line.strip_prefix("data: ") {
                    stripped
                } else {
                    continue;
                };
                if data == "[DONE]" {
                    break;
                }
                append_raw_response(&mut raw_response, data);

                let chunk: StreamChunk = serde_json::from_str(data)
                    .with_context(|| format!("解析 LLM 流式响应失败：{data}"))?;
                let StreamChunk {
                    choices,
                    usage: chunk_usage,
                } = chunk;
                if let Some(chunk_usage) = chunk_usage {
                    usage = Some(chunk_usage);
                }
                let Some(choice) = choices.first() else {
                    continue;
                };

                if let Some(reasoning) = choice.delta.thinking_delta() {
                    if !reasoning.is_empty() {
                        let started_at =
                            thinking_started_at.get_or_insert_with(std::time::Instant::now);
                        thinking_elapsed_ms = started_at.elapsed().as_millis() as u64;
                        thinking_content.push_str(reasoning);
                        on_thinking_delta(reasoning, thinking_elapsed_ms);
                    }
                }

                // Content delta
                if let Some(c) = &choice.delta.content {
                    content.push_str(c);
                    on_delta(c);
                }
                // Tool call fragments
                if let Some(calls) = &choice.delta.tool_calls {
                    for tc in calls {
                        let entry = tc_map
                            .entry(tc.index)
                            .or_insert_with(|| (String::new(), String::new(), String::new()));
                        if let Some(id) = &tc.id {
                            if !id.trim().is_empty() {
                                entry.0.clone_from(id);
                            }
                        }
                        if let Some(f) = &tc.function {
                            if let Some(name) = &f.name {
                                entry.1.clone_from(name);
                            }
                            if let Some(args) = &f.arguments {
                                entry.2.push_str(args);
                            }
                        }
                    }
                }
            }
        }

        let (visible_content, tagged_thinking) = split_tagged_thinking(&content);
        if !tagged_thinking.trim().is_empty() {
            if !thinking_content.trim().is_empty() {
                thinking_content.push_str("\n\n");
            }
            thinking_content.push_str(tagged_thinking.trim());
        }

        // Build final tool calls from accumulated fragments
        let tool_calls = build_requested_tool_calls(tc_map)?;

        Ok(LlmResponse {
            status_code: status.as_u16(),
            content: visible_content,
            thinking_content,
            thinking_elapsed_ms,
            tool_calls,
            raw_response,
            usage,
        })
    }

    fn enable_thinking_parameter(&self, enable_thinking: bool) -> Option<bool> {
        if enable_thinking || should_explicitly_disable_thinking(&self.model, &self.api_base) {
            Some(enable_thinking)
        } else {
            None
        }
    }
}

fn should_explicitly_disable_thinking(model: &str, api_base: &str) -> bool {
    let model = model.to_ascii_lowercase();
    let api_base = api_base.to_ascii_lowercase();
    model.contains("qwen") || model.contains("qwq") || api_base.contains("dashscope")
}

fn split_tagged_thinking(content: &str) -> (String, String) {
    let lower = content.to_ascii_lowercase();
    let mut visible = String::new();
    let mut thinking_blocks = Vec::new();
    let mut cursor = 0usize;

    while let Some(start_rel) = lower[cursor..].find("<think>") {
        let start = cursor + start_rel;
        let body_start = start + "<think>".len();
        let Some(end_rel) = lower[body_start..].find("</think>") else {
            break;
        };
        let end = body_start + end_rel;
        let tag_end = end + "</think>".len();

        visible.push_str(&content[cursor..start]);
        let thinking = content[body_start..end].trim();
        if !thinking.is_empty() {
            thinking_blocks.push(thinking.to_string());
        }
        cursor = tag_end;
    }

    visible.push_str(&content[cursor..]);

    (visible.trim().to_string(), thinking_blocks.join("\n\n"))
}

const MAX_RAW_RESPONSE_CHARS: usize = 20_000;

fn append_raw_response(raw_response: &mut String, data: &str) {
    if raw_response.chars().count() >= MAX_RAW_RESPONSE_CHARS {
        return;
    }
    if !raw_response.is_empty() {
        raw_response.push('\n');
    }
    raw_response.push_str(data);
    *raw_response = truncate_for_display(
        raw_response,
        MAX_RAW_RESPONSE_CHARS,
        "\n...[LLM 原始响应已截断]",
    );
}

fn should_retry_without_stream_options(
    status: StatusCode,
    response_body: &str,
    has_stream_options: bool,
) -> bool {
    has_stream_options
        && status == StatusCode::BAD_REQUEST
        && (response_body.contains("Required body invalid")
            || response_body.contains("stream_options")
            || response_body.contains("request body"))
}

fn build_requested_tool_calls(
    tc_map: BTreeMap<usize, (String, String, String)>,
) -> Result<Vec<RequestedToolCall>> {
    tc_map
        .into_values()
        .map(|(id, name, args)| {
            if id.trim().is_empty() {
                return Err(anyhow!("LLM 工具调用缺少 tool_call id。"));
            }
            if name.trim().is_empty() {
                return Err(anyhow!(
                    "LLM 工具调用缺少 function name，tool_call_id={id}。"
                ));
            }
            let arguments = serde_json::from_str(&args).with_context(|| {
                format!("解析 LLM 工具调用参数失败，tool_call_id={id}, function={name}")
            })?;
            Ok(RequestedToolCall {
                id,
                name,
                arguments,
            })
        })
        .collect()
}

async fn build_api_messages(
    messages: &[ChatMessage],
    enable_multimodal: bool,
) -> Result<Vec<ApiChatMessage>> {
    let mut api_messages = Vec::with_capacity(messages.len());

    for message in messages {
        api_messages.push(ApiChatMessage {
            role: message.role.clone(),
            content: if enable_multimodal && message.role == "user" {
                build_api_message_content(&message.content).await?
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

async fn build_api_message_content(content: &str) -> Result<ApiMessageContent> {
    let parts = match extract_multimodal_parts(content).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "build_api_message_content: extract_multimodal_parts failed for {} byte content: {e:#}",
                content.len()
            );
            return Ok(ApiMessageContent::Text(content.to_string()));
        }
    };
    if parts
        .iter()
        .any(|part| matches!(part, ApiMessageContentPart::ImageUrl { .. }))
    {
        eprintln!(
            "build_api_message_content: multimodal — {} parts ({} image_url, {} text)",
            parts.len(),
            parts
                .iter()
                .filter(|p| matches!(p, ApiMessageContentPart::ImageUrl { .. }))
                .count(),
            parts
                .iter()
                .filter(|p| matches!(p, ApiMessageContentPart::Text { .. }))
                .count(),
        );
        Ok(ApiMessageContent::Parts(parts))
    } else {
        eprintln!(
            "build_api_message_content: no image_url parts found in {} byte content, falling back to text",
            content.len()
        );
        Ok(ApiMessageContent::Text(content.to_string()))
    }
}

fn content_contains_inline_image(content: &str) -> bool {
    extract_inline_image_urls(content).next().is_some()
}

async fn extract_multimodal_parts(content: &str) -> Result<Vec<ApiMessageContentPart>> {
    let mut parts = Vec::new();
    let mut cursor = 0usize;

    for image in extract_inline_image_urls(content) {
        if image.start > cursor {
            push_text_part(&mut parts, &content[cursor..image.start]);
        }
        parts.push(ApiMessageContentPart::ImageUrl {
            image_url: ApiImageUrl {
                url: image_url_for_api(&image.url).await?,
            },
        });
        cursor = image.end;
    }

    if cursor < content.len() {
        push_text_part(&mut parts, &content[cursor..]);
    }

    Ok(parts)
}

fn push_text_part(parts: &mut Vec<ApiMessageContentPart>, text: &str) {
    let trimmed = text.trim();
    if !trimmed.is_empty() {
        parts.push(ApiMessageContentPart::Text {
            text: trimmed.to_string(),
        });
    }
}

struct InlineImageUrl {
    start: usize,
    end: usize,
    url: String,
}

fn extract_inline_image_urls(content: &str) -> impl Iterator<Item = InlineImageUrl> + '_ {
    InlineImageUrlIter {
        content,
        search_from: 0,
    }
}

struct InlineImageUrlIter<'a> {
    content: &'a str,
    search_from: usize,
}

impl Iterator for InlineImageUrlIter<'_> {
    type Item = InlineImageUrl;

    fn next(&mut self) -> Option<Self::Item> {
        while self.search_from < self.content.len() {
            let rel_start = self.content[self.search_from..].find("![")?;
            let start = self.search_from + rel_start;
            let after_bang = start + 2;
            let Some(alt_end_rel) = self.content[after_bang..].find("](") else {
                self.search_from = after_bang;
                continue;
            };
            let url_start = after_bang + alt_end_rel + 2;
            let Some(url_end_rel) = self.content[url_start..].find(')') else {
                self.search_from = url_start;
                continue;
            };
            let url_end = url_start + url_end_rel;
            let end = url_end + 1;
            let url = &self.content[url_start..url_end];
            self.search_from = end;

            if is_supported_image_reference(url) {
                return Some(InlineImageUrl {
                    start,
                    end,
                    url: url.to_string(),
                });
            }
        }

        None
    }
}

fn is_supported_image_reference(url: &str) -> bool {
    url.starts_with("data:image/")
        || url.starts_with(crate::chat_images::CHAT_IMAGE_PROTOCOL)
        || local_image_path(url).is_ok()
}

async fn image_url_for_api(url: &str) -> Result<String> {
    if url.starts_with("data:image/") {
        return Ok(url.to_string());
    }

    // `chat-image://<image_id>` URIs must be resolved to their filesystem path
    // first (the path is stored in `~/.jkcodingagent/chat-images/...` and is
    // looked up by scanning that directory). Only then can it be read and
    // encoded as a data URL for the LLM vision API.
    if url.starts_with(crate::chat_images::CHAT_IMAGE_PROTOCOL) {
        let resolved = crate::chat_images::resolve_chat_image_id_async(url.to_string())
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        return tokio::task::spawn_blocking(move || local_image_to_data_url(&resolved))
            .await
            .context("读取 chat-image 图片任务失败")?;
    }

    let path = local_image_path(url)?;
    tokio::task::spawn_blocking(move || local_image_to_data_url(&path))
        .await
        .context("读取本地图片任务失败")?
}

fn local_image_path(url: &str) -> Result<PathBuf> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        anyhow::bail!("图片路径为空");
    }

    let path = Path::new(trimmed);
    if !path.is_absolute() {
        anyhow::bail!("图片路径必须是绝对路径：{trimmed}");
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        anyhow::bail!("图片路径不能包含父目录跳转：{trimmed}");
    }

    image_mime_from_path(path)?;
    Ok(path.to_path_buf())
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
        .unwrap_or_default()
        .to_ascii_lowercase();

    match ext.as_str() {
        "png" => Ok("image/png"),
        "jpg" | "jpeg" => Ok("image/jpeg"),
        "webp" => Ok("image/webp"),
        "gif" => Ok("image/gif"),
        _ => anyhow::bail!("不支持的图片格式：{}", path.display()),
    }
}

/// Fetch model list from an OpenAI-compatible `/v1/models` endpoint.
pub async fn fetch_models(api_base: &str, api_key: &str) -> Result<Vec<String>> {
    let base_url = format!("{}/models", api_base.trim_end_matches('/'));
    let client = Client::new();

    let response = client
        .get(&base_url)
        .bearer_auth(api_key)
        .send()
        .await
        .context("获取模型列表请求失败")?;

    let status = response.status();
    let raw = response.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err(anyhow!("获取模型列表失败，HTTP {}：{}", status, raw));
    }

    // Try standard OpenAI format: { "data": [{ "id": "..." }] }
    if let Ok(parsed) = serde_json::from_str::<OpenAiModelsResponse>(&raw) {
        let mut ids: Vec<String> = parsed.data.into_iter().map(|m| m.id).collect();
        ids.sort();
        ids.dedup();
        return Ok(ids);
    }

    // Try DashScope format
    if let Ok(parsed) = serde_json::from_str::<DashScopeModelsResponse>(&raw) {
        let mut ids: Vec<String> = parsed.output.models.into_iter().map(|m| m.model).collect();

        let total = parsed.output.total;
        let page_size = parsed.output.page_size.max(1);
        let total_pages = total.div_ceil(page_size);

        for page in 2..=total_pages.min(30) {
            let page_url = format!("{}?page_no={}&page_size={}", base_url, page, page_size);
            if let Ok(resp) = client.get(&page_url).bearer_auth(api_key).send().await {
                if let Ok(body) = resp.text().await {
                    if let Ok(p) = serde_json::from_str::<DashScopeModelsResponse>(&body) {
                        ids.extend(p.output.models.into_iter().map(|m| m.model));
                    }
                }
            }
        }

        ids.sort();
        ids.dedup();
        return Ok(ids);
    }

    // Fallback: try as plain array
    if let Ok(arr) = serde_json::from_str::<Vec<OpenAiModelEntry>>(&raw) {
        let mut ids: Vec<String> = arr.into_iter().map(|m| m.id).collect();
        ids.sort();
        ids.dedup();
        return Ok(ids);
    }

    let preview = truncate_for_display(&raw, 500, "");
    Err(anyhow!("无法解析模型列表响应，原始内容:\n{}", preview))
}

// --- OpenAI standard format ---
#[derive(Deserialize)]
struct OpenAiModelsResponse {
    data: Vec<OpenAiModelEntry>,
}

#[derive(Deserialize)]
struct OpenAiModelEntry {
    id: String,
}

// --- DashScope format ---
#[derive(Deserialize)]
struct DashScopeModelsResponse {
    output: DashScopeOutput,
}

#[derive(Deserialize)]
struct DashScopeOutput {
    models: Vec<DashScopeModelEntry>,
    total: usize,
    page_size: usize,
}

#[derive(Deserialize)]
struct DashScopeModelEntry {
    model: String,
}

// --- Streaming request ---
#[derive(Serialize)]
struct StreamChatRequest<'a> {
    model: &'a str,
    messages: &'a [ApiChatMessage],
    max_tokens: u32,
    temperature: f32,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'a [ToolDefinition]>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StreamOptions {
    include_usage: bool,
}

// --- SSE stream chunk types ---
#[derive(Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
    #[serde(default)]
    usage: Option<LlmUsage>,
}

#[derive(Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
}

#[derive(Deserialize)]
struct StreamDelta {
    content: Option<String>,
    reasoning_content: Option<String>,
    reasoning: Option<String>,
    thinking: Option<String>,
    tool_calls: Option<Vec<StreamToolCall>>,
}

impl StreamDelta {
    fn thinking_delta(&self) -> Option<&str> {
        self.reasoning_content
            .as_deref()
            .or(self.reasoning.as_deref())
            .or(self.thinking.as_deref())
    }
}

#[derive(Deserialize)]
struct StreamToolCall {
    index: usize,
    id: Option<String>,
    function: Option<StreamFunctionCall>,
}

#[derive(Deserialize)]
struct StreamFunctionCall {
    name: Option<String>,
    arguments: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use base64::Engine;
    use reqwest::StatusCode;

    use super::{
        append_raw_response, append_valid_utf8, build_api_message_content, build_api_messages,
        build_requested_tool_calls, is_supported_image_reference, messages_contain_inline_images,
        should_explicitly_disable_thinking, should_retry_without_stream_options,
        split_tagged_thinking, ApiMessageContent, ApiMessageContentPart, ChatMessage,
        OpenAiCompatProvider, StreamChunk,
    };

    #[test]
    fn stream_chunk_supports_usage_only_tail_event() {
        let chunk = serde_json::from_str::<StreamChunk>(
            r#"{
                "choices": [],
                "usage": {
                    "prompt_tokens": 321,
                    "completion_tokens": 45,
                    "total_tokens": 366,
                    "prompt_tokens_details": {
                        "cached_tokens": 128
                    }
                }
            }"#,
        )
        .expect("parse usage tail chunk");

        assert!(chunk.choices.is_empty());
        let usage = chunk.usage.expect("usage should exist");
        assert_eq!(usage.prompt_tokens, 321);
        assert_eq!(usage.completion_tokens, 45);
        assert_eq!(usage.total_tokens, 366);
        assert_eq!(usage.cached_tokens(), 128);
    }

    #[test]
    fn stream_chunk_supports_reasoning_content_delta() {
        let chunk = serde_json::from_str::<StreamChunk>(
            r#"{
                "choices": [{
                    "delta": {
                        "reasoning_content": "先拆问题",
                        "content": null
                    }
                }]
            }"#,
        )
        .expect("parse reasoning chunk");

        let delta = &chunk.choices.first().expect("choice").delta;
        assert_eq!(delta.thinking_delta(), Some("先拆问题"));
    }

    #[test]
    fn split_tagged_thinking_removes_complete_think_blocks() {
        let (visible, thinking) =
            split_tagged_thinking("开头\n<think>先拆问题</think>\n结论\n<THINK>再校验</THINK>");

        assert_eq!(visible, "开头\n\n结论");
        assert_eq!(thinking, "先拆问题\n\n再校验");
    }

    #[test]
    fn retries_body_format_errors_without_stream_options_only_when_relevant() {
        assert!(should_retry_without_stream_options(
            StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"Required body invalid, please check the request body format."}}"#,
            true,
        ));
        assert!(!should_retry_without_stream_options(
            StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"Required body invalid, please check the request body format."}}"#,
            false,
        ));
        assert!(!should_retry_without_stream_options(
            StatusCode::UNAUTHORIZED,
            r#"{"error":{"message":"Required body invalid"}}"#,
            true,
        ));
    }

    #[test]
    fn qwen_provider_explicitly_disables_thinking_when_requested_off() {
        assert!(should_explicitly_disable_thinking(
            "o-qwen3.7-plus",
            "http://10.144.144.2:8317/v1"
        ));
        assert!(should_explicitly_disable_thinking(
            "custom-model",
            "https://dashscope.aliyuncs.com/compatible-mode/v1"
        ));
        assert!(!should_explicitly_disable_thinking(
            "gpt-4.1",
            "https://api.openai.com/v1"
        ));
    }

    #[test]
    fn request_snapshot_includes_false_thinking_for_qwen_only() {
        let messages = vec![ChatMessage::system("只输出 pong。".to_string())];
        let qwen = OpenAiCompatProvider::new(
            "test-key".to_string(),
            "http://localhost:8317/v1".to_string(),
            "o-qwen3.7-plus".to_string(),
            64,
            0.0,
        );
        let openai = OpenAiCompatProvider::new(
            "test-key".to_string(),
            "https://api.openai.com/v1".to_string(),
            "gpt-4.1".to_string(),
            64,
            0.0,
        );

        assert_eq!(
            qwen.build_request_snapshot(&messages, &[], false)
                .body
                .enable_thinking,
            Some(false)
        );
        assert_eq!(
            openai
                .build_request_snapshot(&messages, &[], false)
                .body
                .enable_thinking,
            None
        );
    }

    #[test]
    fn keeps_raw_stream_payload_for_diagnostics() {
        let mut raw_response = String::new();

        append_raw_response(&mut raw_response, r#"{"choices":[]}"#);
        append_raw_response(
            &mut raw_response,
            r#"{"usage":{"prompt_tokens":1,"completion_tokens":0,"total_tokens":1}}"#,
        );

        assert!(raw_response.contains(r#""choices":[]"#));
        assert!(raw_response.contains(r#""usage""#));
        assert_eq!(raw_response.lines().count(), 2);
    }

    #[test]
    fn requested_tool_calls_fail_on_invalid_arguments() {
        let mut calls = BTreeMap::new();
        calls.insert(
            0,
            (
                "call_1".to_string(),
                "create_plan_document".to_string(),
                "{not json}".to_string(),
            ),
        );

        let error = build_requested_tool_calls(calls).expect_err("invalid args should fail");
        assert!(error.to_string().contains("解析 LLM 工具调用参数失败"));
    }

    #[test]
    fn requested_tool_calls_fail_on_missing_identity() {
        let mut missing_id = BTreeMap::new();
        missing_id.insert(
            0,
            (
                String::new(),
                "create_plan_document".to_string(),
                "{}".to_string(),
            ),
        );
        assert!(build_requested_tool_calls(missing_id)
            .expect_err("missing id should fail")
            .to_string()
            .contains("缺少 tool_call id"));

        let mut missing_name = BTreeMap::new();
        missing_name.insert(0, ("call_1".to_string(), String::new(), "{}".to_string()));
        assert!(build_requested_tool_calls(missing_name)
            .expect_err("missing name should fail")
            .to_string()
            .contains("缺少 function name"));
    }

    #[test]
    fn detects_inline_images_only_in_user_messages() {
        let messages = vec![
            ChatMessage {
                role: "assistant".to_string(),
                content: "![image](data:image/png;base64,aaa)".to_string(),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "user".to_string(),
                content: "看这里 ![image](data:image/jpeg;base64,bbb)".to_string(),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
        ];

        assert!(messages_contain_inline_images(&messages));
    }

    #[test]
    fn detects_local_markdown_images_in_user_messages() {
        let image_path = std::env::temp_dir()
            .join("jkcodingagent-vision-detect.png")
            .to_string_lossy()
            .to_string();
        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: format!("看这里 ![image]({image_path})"),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }];

        assert!(messages_contain_inline_images(&messages));
    }

    #[tokio::test]
    async fn api_messages_preserve_assistant_reasoning_content_only() {
        let messages = vec![
            ChatMessage {
                role: "assistant".to_string(),
                content: String::new(),
                reasoning_content: Some("需要先调用工具。".to_string()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "user".to_string(),
                content: "继续".to_string(),
                reasoning_content: Some("不应发送".to_string()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
        ];

        let api_messages = build_api_messages(&messages, false).await.unwrap();
        let json = serde_json::to_value(&api_messages).unwrap();

        assert_eq!(json[0]["reasoning_content"], "需要先调用工具。");
        assert!(json[1].get("reasoning_content").is_none());
    }

    #[test]
    fn utf8_stream_decoder_keeps_split_multibyte_tail() {
        let mut buffer = String::new();
        let mut leftover = Vec::new();
        let bytes = "data: 你好\n".as_bytes();

        append_valid_utf8(&mut buffer, &mut leftover, &bytes[..8]);
        assert_eq!(buffer, "data: ");
        assert!(!leftover.is_empty());

        append_valid_utf8(&mut buffer, &mut leftover, &bytes[8..]);
        assert_eq!(buffer, "data: 你好\n");
        assert!(leftover.is_empty());
    }

    #[test]
    fn utf8_stream_decoder_drops_invalid_bytes_without_losing_following_text() {
        let mut buffer = String::new();
        let mut leftover = Vec::new();

        append_valid_utf8(&mut buffer, &mut leftover, b"data: ok ");
        append_valid_utf8(&mut buffer, &mut leftover, &[0xF0]);
        append_valid_utf8(&mut buffer, &mut leftover, b"after\n");

        assert_eq!(buffer, "data: ok after\n");
        assert!(leftover.is_empty());
    }

    #[tokio::test]
    async fn converts_markdown_data_images_to_openai_parts() {
        let content = "先看图\n![image](data:image/png;base64,aaa)\n再解释";
        let ApiMessageContent::Parts(parts) = build_api_message_content(content).await.unwrap()
        else {
            panic!("expected multimodal parts");
        };

        assert_eq!(parts.len(), 3);
        assert!(matches!(parts[0], ApiMessageContentPart::Text { .. }));
        assert!(matches!(parts[1], ApiMessageContentPart::ImageUrl { .. }));
        assert!(matches!(parts[2], ApiMessageContentPart::Text { .. }));
    }

    #[tokio::test]
    async fn converts_local_markdown_images_to_data_url_parts() {
        let dir = std::env::temp_dir().join(format!(
            "jkcodingagent-vision-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("pasted.png");
        std::fs::write(&path, b"image-bytes").expect("write fake image bytes");
        let content = format!("先看图\n![image]({})\n再解释", path.display());

        let ApiMessageContent::Parts(parts) = build_api_message_content(&content).await.unwrap()
        else {
            panic!("expected multimodal parts");
        };

        assert_eq!(parts.len(), 3);
        let ApiMessageContentPart::ImageUrl { image_url } = &parts[1] else {
            panic!("expected image_url part");
        };
        assert!(image_url.url.starts_with("data:image/png;base64,"));
        assert!(!image_url.url.contains(path.to_string_lossy().as_ref()));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn chat_image_protocol_is_supported_image_reference() {
        assert!(is_supported_image_reference(
            "chat-image://550e8400-e29b-41d4-a716-446655440000"
        ));
    }

    #[tokio::test]
    async fn chat_image_protocol_resolved_to_data_url() {
        use crate::chat_images::save_chat_image;

        let small = make_small_test_png();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&small);
        let saved = save_chat_image(
            "s".to_string(),
            "llm-chat-image-test".to_string(),
            b64,
            "image/png".to_string(),
        )
        .await
        .expect("save chat image");

        let content = format!("看图\n![image](chat-image://{})\n请描述", saved.image_id);
        let ApiMessageContent::Parts(parts) = build_api_message_content(&content).await.unwrap()
        else {
            panic!("expected multimodal parts, got text fallback");
        };

        assert_eq!(parts.len(), 3);
        let ApiMessageContentPart::ImageUrl { image_url } = &parts[1] else {
            panic!("expected image_url part");
        };
        assert!(
            image_url.url.starts_with("data:image/png;base64,"),
            "chat-image:// must be resolved to a data URL, got: {}",
            &image_url.url[..image_url.url.len().min(80)]
        );

        let file_path = std::path::PathBuf::from(&saved.path);
        if let Some(parent) = file_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    fn make_small_test_png() -> Vec<u8> {
        let img = image::RgbaImage::from_fn(8, 8, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 255])
        });
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .expect("encode test png");
        buf.into_inner()
    }
}

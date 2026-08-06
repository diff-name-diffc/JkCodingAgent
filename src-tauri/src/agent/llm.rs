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
    /// Plain text view used by summaries, logs, and text-only providers.
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_parts: Vec<ChatMessageContentPart>,
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
            content_parts: Vec::new(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatMessageContentPart {
    Text { text: String },
    Image { source: ChatMessageImageSource },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatMessageImageSource {
    ChatImage { image_id: String },
    DataUrl { data_url: String },
    LocalPath { path: String },
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
    /// 流式响应的终止原因（stop/length/tool_calls/…）。
    /// `length` 表示输出被 max_tokens 截断——推理模型思考链耗尽预算时
    /// 可见内容可能为空，调用方据此区分「截断」与「模型未遵循格式」。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
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
    pub enable_thinking: bool,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
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

pub fn messages_contain_images(messages: &[ChatMessage]) -> bool {
    messages.iter().any(|message| {
        message.role == "user"
            && message
                .content_parts
                .iter()
                .any(|part| matches!(part, ChatMessageContentPart::Image { .. }))
    })
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
    /// 是否允许模型输出思考链（推理模型）。默认 true 保持既有行为；
    /// 短结论任务（如验收评审、格式分类）应显式关闭，避免思考 token
    /// 与可见输出共享 max_tokens 预算时把结论挤掉。
    enable_thinking: bool,
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
            enable_thinking: true,
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

    /// 覆盖思考策略（见 `enable_thinking` 字段注释）。链式用法：
    /// `OpenAiCompatProvider::new(...).with_thinking(false)`。
    pub fn with_thinking(&self, enable_thinking: bool) -> Self {
        let mut next = self.clone();
        next.enable_thinking = enable_thinking;
        next
    }

    pub fn build_request_snapshot(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
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
                enable_thinking: self.enable_thinking,
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
                finish_reason: response.finish_reason.clone(),
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
        self.chat_stream_with_thinking(messages, tools, enable_multimodal, on_delta, |_, _| {})
            .await
    }

    /// Streaming chat completion. Thinking output follows the model/provider default behavior.
    pub async fn chat_stream_with_thinking(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        enable_multimodal: bool,
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
            enable_thinking: self.enable_thinking,
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
            .with_context(|| format!("发送流式对话请求失败：model={} url={}", self.model, url))?;

        let mut status = response.status();
        let response = if status.is_success() {
            response
        } else {
            let initial_status = status;
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
                    .with_context(|| {
                        format!(
                            "发送无 stream_options 的流式对话重试请求失败：model={} url={}",
                            self.model, url
                        )
                    })?;
                status = retry_response.status();
                if status.is_success() {
                    retry_response
                } else {
                    let retry_body = retry_response
                        .text()
                        .await
                        .context("读取 LLM 重试错误响应失败")?;
                    return Err(anyhow!(
                        "{}；去除 stream_options 后仍失败：{}",
                        format_llm_http_error(initial_status, &body),
                        format_llm_http_error(status, &retry_body)
                    ));
                }
            } else {
                return Err(anyhow!("{}", format_llm_http_error(initial_status, &body)));
            }
        };

        if !status.is_success() {
            let body = response.text().await.context("读取 LLM 错误响应失败")?;
            return Err(anyhow!("{}", format_llm_http_error(status, &body)));
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
        let mut finish_reason: Option<String> = None;
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

                // 终止原因（stop/length/…）：部分服务在最后一个分片才给出。
                if let Some(reason) = choice
                    .finish_reason
                    .as_deref()
                    .map(str::trim)
                    .filter(|reason| !reason.is_empty())
                {
                    finish_reason = Some(reason.to_string());
                }

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
            finish_reason,
        })
    }
}

fn format_llm_http_error(status: StatusCode, body: &str) -> String {
    let body = body.trim();
    let detail = if body.is_empty() {
        "<空响应体>".to_string()
    } else {
        truncate_for_display(body, 4_000, "\n...[LLM 错误响应已截断]")
    };
    format!("LLM 请求失败，HTTP {}：{}", status, detail)
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

async fn build_api_message_content(message: &ChatMessage) -> Result<ApiMessageContent> {
    let mut parts = Vec::new();

    for part in &message.content_parts {
        match part {
            ChatMessageContentPart::Text { text } => push_text_part(&mut parts, text),
            ChatMessageContentPart::Image { source } => {
                parts.push(ApiMessageContentPart::ImageUrl {
                    image_url: ApiImageUrl {
                        url: image_source_for_api(source).await?,
                    },
                });
            }
        }
    }

    if parts
        .iter()
        .any(|part| matches!(part, ApiMessageContentPart::ImageUrl { .. }))
    {
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

async fn image_source_for_api(source: &ChatMessageImageSource) -> Result<String> {
    match source {
        ChatMessageImageSource::DataUrl { data_url } => {
            if !data_url.starts_with("data:image/") {
                anyhow::bail!("图片 data URL 必须以 data:image/ 开头");
            }
            Ok(data_url.clone())
        }
        ChatMessageImageSource::ChatImage { image_id } => {
            let resolved = crate::chat_images::resolve_chat_image_id_async(image_id.clone())
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            tokio::task::spawn_blocking(move || local_image_to_data_url(&resolved))
                .await
                .context("读取聊天图片任务失败")?
        }
        ChatMessageImageSource::LocalPath { path } => {
            let path = local_image_path(path)?;
            tokio::task::spawn_blocking(move || local_image_to_data_url(&path))
                .await
                .context("读取本地图片任务失败")?
        }
    }
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
    enable_thinking: bool,
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
    #[serde(default)]
    finish_reason: Option<String>,
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

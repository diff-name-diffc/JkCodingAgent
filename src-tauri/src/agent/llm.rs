use std::collections::BTreeMap;

use anyhow::{anyhow, Context, Result};
use futures::StreamExt;
use reqwest::Client;
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
    pub tool_calls: Vec<RequestedToolCall>,
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
    pub tool_calls: Vec<RequestedToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<LlmUsage>,
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
            client: Client::new(),
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

    pub fn with_model(&self, model: impl Into<String>) -> Self {
        let mut next = self.clone();
        next.model = model.into();
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
                tool_calls: response.tool_calls.clone(),
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
        mut on_delta: impl FnMut(&str),
    ) -> Result<LlmResponse> {
        if !self.is_configured() {
            return Err(anyhow!("LLM API Key 尚未配置。"));
        }

        let url = format!("{}/chat/completions", self.api_base.trim_end_matches('/'));
        let api_messages = build_api_messages(messages, enable_multimodal);
        let request = StreamChatRequest {
            model: &self.model,
            messages: &api_messages,
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            tools: if tools.is_empty() { None } else { Some(tools) },
            stream: true,
            stream_options: Some(StreamOptions {
                include_usage: true,
            }),
        };

        let response = self
            .client
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await
            .context("发送流式对话请求失败")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.context("读取 LLM 错误响应失败")?;
            return Err(anyhow!("LLM 请求失败，HTTP {}：{}", status, body));
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut content = String::new();
        let mut usage: Option<LlmUsage> = None;
        // index -> (id, name, accumulated_arguments)
        let mut tc_map: BTreeMap<usize, (String, String, String)> = BTreeMap::new();

        while let Some(chunk) = stream.next().await {
            let bytes = chunk.context("读取流式响应分片失败")?;
            buffer.push_str(&String::from_utf8_lossy(&bytes));

            // Process complete lines from the buffer
            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim_end().to_string();
                buffer = buffer[newline_pos + 1..].to_string();

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

        // Build final tool calls from accumulated fragments
        let tool_calls = build_requested_tool_calls(tc_map)?;

        Ok(LlmResponse {
            status_code: status.as_u16(),
            content,
            tool_calls,
            usage,
        })
    }
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

fn build_api_messages(messages: &[ChatMessage], enable_multimodal: bool) -> Vec<ApiChatMessage> {
    messages
        .iter()
        .map(|message| ApiChatMessage {
            role: message.role.clone(),
            content: if enable_multimodal && message.role == "user" {
                build_api_message_content(&message.content)
            } else {
                ApiMessageContent::Text(message.content.clone())
            },
            tool_calls: message.tool_calls.clone(),
            tool_call_id: message.tool_call_id.clone(),
            name: message.name.clone(),
        })
        .collect()
}

fn build_api_message_content(content: &str) -> ApiMessageContent {
    let parts = extract_multimodal_parts(content);
    if parts
        .iter()
        .any(|part| matches!(part, ApiMessageContentPart::ImageUrl { .. }))
    {
        ApiMessageContent::Parts(parts)
    } else {
        ApiMessageContent::Text(content.to_string())
    }
}

fn content_contains_inline_image(content: &str) -> bool {
    extract_inline_image_urls(content).next().is_some()
}

fn extract_multimodal_parts(content: &str) -> Vec<ApiMessageContentPart> {
    let mut parts = Vec::new();
    let mut cursor = 0usize;

    for image in extract_inline_image_urls(content) {
        if image.start > cursor {
            push_text_part(&mut parts, &content[cursor..image.start]);
        }
        parts.push(ApiMessageContentPart::ImageUrl {
            image_url: ApiImageUrl { url: image.url },
        });
        cursor = image.end;
    }

    if cursor < content.len() {
        push_text_part(&mut parts, &content[cursor..]);
    }

    parts
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

            if url.starts_with("data:image/") {
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
        let total_pages = (total + page_size - 1) / page_size;

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
    tool_calls: Option<Vec<StreamToolCall>>,
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

    use super::{
        build_api_message_content, build_requested_tool_calls, messages_contain_inline_images,
        ApiMessageContent, ApiMessageContentPart, ChatMessage, StreamChunk,
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
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "user".to_string(),
                content: "看这里 ![image](data:image/jpeg;base64,bbb)".to_string(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
        ];

        assert!(messages_contain_inline_images(&messages));
    }

    #[test]
    fn converts_markdown_data_images_to_openai_parts() {
        let content = "先看图\n![image](data:image/png;base64,aaa)\n再解释";
        let ApiMessageContent::Parts(parts) = build_api_message_content(content) else {
            panic!("expected multimodal parts");
        };

        assert_eq!(parts.len(), 3);
        assert!(matches!(parts[0], ApiMessageContentPart::Text { .. }));
        assert!(matches!(parts[1], ApiMessageContentPart::ImageUrl { .. }));
        assert!(matches!(parts[2], ApiMessageContentPart::Text { .. }));
    }
}

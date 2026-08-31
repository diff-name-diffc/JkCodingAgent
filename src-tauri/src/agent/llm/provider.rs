use std::collections::BTreeMap;

use anyhow::{anyhow, Context, Result};
use futures::StreamExt;
use reqwest::Client;

use super::protocol::{
    append_raw_response, append_valid_utf8, build_requested_tool_calls, format_llm_http_error,
    parse_sse_data_line, should_retry_without_extra_fields, split_tagged_thinking,
    StreamChatRequest, StreamChunk,
};
use super::request::build_api_messages;
use super::{
    ChatMessage, LlmRequestBodySnapshot, LlmRequestHeadersSnapshot, LlmRequestSnapshot,
    LlmResponse, LlmResponseBodySnapshot, LlmResponseSnapshot, LlmUsage, StreamOptions,
    ToolDefinition,
};

#[derive(Clone)]
pub struct OpenAiCompatProvider {
    client: Client,
    api_key: String,
    api_base: String,
    model: String,
    /// 输出上限。None 时请求体完全省略 max_tokens，由服务端默认预算接管
    /// （见 `without_max_tokens`）。
    max_tokens: Option<u32>,
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
            max_tokens: Some(max_tokens),
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

    /// 不设输出上限：请求体完全省略 max_tokens 字段，由服务端默认预算接管。
    /// 适用于模型支持超大输出预算的场景——显式传一个小上限反而会与服务端
    /// 预算互相挤压（推理模型的思考 token 还会与可见输出共享该预算）。
    /// 链式用法：`OpenAiCompatProvider::new(..., 0, temperature).without_max_tokens()`。
    pub fn without_max_tokens(&self) -> Self {
        let mut next = self.clone();
        next.max_tokens = None;
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
            enable_thinking: Some(self.enable_thinking),
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
            if should_retry_without_extra_fields(
                status,
                &body,
                request.stream_options.is_some(),
                request.enable_thinking.is_some(),
            ) {
                // 严格的 OpenAI 兼容服务会拒绝 stream_options / enable_thinking
                // 这类非标准字段，一并移除后重试。
                request.stream_options = None;
                request.enable_thinking = None;
                let retry_response = self
                    .client
                    .post(&url)
                    .bearer_auth(&self.api_key)
                    .json(&request)
                    .send()
                    .await
                    .with_context(|| {
                        format!(
                            "发送移除附加字段后的流式对话重试请求失败：model={} url={}",
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
                        "{}；去除 stream_options/enable_thinking 后仍失败：{}",
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
                let Some(data) = parse_sse_data_line(&line) else {
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

use std::collections::BTreeMap;

use anyhow::{anyhow, Context, Result};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::shared::truncate_for_display;

use super::request::ApiChatMessage;
use super::{LlmUsage, RequestedToolCall, ToolDefinition};

pub(super) fn append_valid_utf8(buffer: &mut String, leftover_bytes: &mut Vec<u8>, bytes: &[u8]) {
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

pub(super) fn format_llm_http_error(status: StatusCode, body: &str) -> String {
    let body = body.trim();
    let detail = if body.is_empty() {
        "<空响应体>".to_string()
    } else {
        truncate_for_display(body, 4_000, "\n...[LLM 错误响应已截断]")
    };
    format!("LLM 请求失败，HTTP {}：{}", status, detail)
}

/// 解析 SSE 行的 `data:` 负载。按 SSE 规范 `data:` 后可无空格
/// （部分 OpenAI 兼容服务输出 `data:{...}`），两种格式都要兼容；
/// 非 data 行或空负载返回 None。
pub(super) fn parse_sse_data_line(line: &str) -> Option<&str> {
    let data = line.strip_prefix("data:")?.trim_start();
    if data.is_empty() {
        None
    } else {
        Some(data)
    }
}

pub(super) fn split_tagged_thinking(content: &str) -> (String, String) {
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

pub(super) fn append_raw_response(raw_response: &mut String, data: &str) {
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

/// 400 错误是否应去掉非标准/可选字段（stream_options、enable_thinking）后重试。
/// OpenAI 官方及部分严格的 OpenAI 兼容服务会对未知请求参数直接返回 400。
pub(super) fn should_retry_without_extra_fields(
    status: StatusCode,
    response_body: &str,
    has_stream_options: bool,
    has_enable_thinking: bool,
) -> bool {
    if status != StatusCode::BAD_REQUEST || (!has_stream_options && !has_enable_thinking) {
        return false;
    }
    response_body.contains("Required body invalid")
        || response_body.contains("stream_options")
        || response_body.contains("enable_thinking")
        || response_body.contains("request body")
        || response_body.contains("Unknown parameter")
        || response_body.contains("Unsupported parameter")
}

pub(super) fn build_requested_tool_calls(
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

// --- Streaming request ---
#[derive(Serialize)]
pub(super) struct StreamChatRequest<'a> {
    pub(super) model: &'a str,
    pub(super) messages: &'a [ApiChatMessage],
    /// None 时省略 max_tokens，输出上限交给服务端默认预算接管
    /// （配合 provider 的 `without_max_tokens` 路径）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) max_tokens: Option<u32>,
    pub(super) temperature: f32,
    pub(super) stream: bool,
    /// 非标准字段：OpenAI 官方等严格兼容服务会因未知参数返回 400，
    /// 故改为 Option 并在 400 重试路径中置 None（见 should_retry_without_extra_fields）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) enable_thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tools: Option<&'a [ToolDefinition]>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StreamOptions {
    pub(super) include_usage: bool,
}

// --- SSE stream chunk types ---
#[derive(Deserialize)]
pub(super) struct StreamChunk {
    #[serde(default)]
    pub(super) choices: Vec<StreamChoice>,
    #[serde(default)]
    pub(super) usage: Option<LlmUsage>,
}

#[derive(Deserialize)]
pub(super) struct StreamChoice {
    pub(super) delta: StreamDelta,
    #[serde(default)]
    pub(super) finish_reason: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct StreamDelta {
    pub(super) content: Option<String>,
    reasoning_content: Option<String>,
    reasoning: Option<String>,
    thinking: Option<String>,
    pub(super) tool_calls: Option<Vec<StreamToolCall>>,
}

impl StreamDelta {
    pub(super) fn thinking_delta(&self) -> Option<&str> {
        self.reasoning_content
            .as_deref()
            .or(self.reasoning.as_deref())
            .or(self.thinking.as_deref())
    }
}

#[derive(Deserialize)]
pub(super) struct StreamToolCall {
    pub(super) index: usize,
    pub(super) id: Option<String>,
    pub(super) function: Option<StreamFunctionCall>,
}

#[derive(Deserialize)]
pub(super) struct StreamFunctionCall {
    pub(super) name: Option<String>,
    pub(super) arguments: Option<String>,
}

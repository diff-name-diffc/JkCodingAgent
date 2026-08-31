use serde::{Deserialize, Serialize};
use serde_json::Value;

mod models;
mod protocol;
mod provider;
mod request;

pub use models::fetch_models;
pub use protocol::StreamOptions;
pub use provider::OpenAiCompatProvider;

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

    pub fn user(content: String) -> Self {
        Self {
            role: "user".to_string(),
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
    /// None 表示请求体省略 max_tokens（见 provider 的 `without_max_tokens`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
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

/// 本轮工具图片附加上限：每次请求最多内联的最近引用数（防 base64 请求
/// 膨胀；更早的引用模型仍可用 analyze_image 按需查看）。
pub(crate) const MAX_TURN_TOOL_IMAGE_ATTACHMENTS: usize = 3;

/// 把「本轮（最后一条用户消息之后）assistant/tool 消息文本里引用的
/// `chat-image://{image_id}`」附加为该用户消息的视觉输入 parts：
/// fetch_image / generate_image / edit_image / MCP 结果中的图片因此能被
/// 主模型直看，`messages_contain_images` 随之命中、vision 槽位自动切换。
///
/// 每次请求从引用文本重算、返回新列表（不改动持久历史）：已在 user
/// 消息 parts 中的图片（粘贴 + 上次附加）被去重，跨迭代稳定不累积；
/// 最后一条用户消息之前的引用属于历史轮次，不附加。parts 只挂 user
/// 角色——与 `build_api_messages` 的多模态构造约束一致（OpenAI 兼容
/// 网关对 tool 角色携带图片的支持参差）。
pub(crate) fn attach_turn_tool_images(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    let Some(last_user_index) = messages.iter().rposition(|m| m.role == "user") else {
        return messages.to_vec();
    };

    let attached: std::collections::HashSet<&str> = messages[last_user_index]
        .content_parts
        .iter()
        .filter_map(|part| match part {
            ChatMessageContentPart::Image { source } => match source {
                ChatMessageImageSource::ChatImage { image_id } => Some(image_id.as_str()),
                ChatMessageImageSource::DataUrl { .. } => None,
            },
            ChatMessageContentPart::Text { .. } => None,
        })
        .collect();

    // 从最新消息向前收集本轮新引用：越靠后（越新）的引用优先占用上限。
    let mut new_ids: Vec<String> = Vec::new();
    for message in messages[last_user_index + 1..].iter().rev() {
        for id in extract_chat_image_references(&message.content)
            .into_iter()
            .rev()
        {
            if !attached.contains(id.as_str()) && !new_ids.contains(&id) {
                new_ids.push(id);
            }
        }
    }
    if new_ids.is_empty() {
        return messages.to_vec();
    }
    new_ids.truncate(MAX_TURN_TOOL_IMAGE_ATTACHMENTS);

    let mut messages = messages.to_vec();
    let user_message = &mut messages[last_user_index];
    user_message
        .content_parts
        .push(ChatMessageContentPart::Text {
            text: format!(
            "[以下 {} 张图片由本轮工具调用（fetch_image / generate_image / edit_image 等）产生的 \
             chat-image:// 引用附加为视觉输入]",
            new_ids.len()
        ),
        });
    for id in new_ids {
        user_message
            .content_parts
            .push(ChatMessageContentPart::Image {
                source: ChatMessageImageSource::ChatImage { image_id: id },
            });
    }
    messages
}

/// 从纯文本里按出现顺序抽取 `chat-image://{id}` 引用。id 形态与
/// `chat-image` scheme handler 的白名单一致（`[0-9A-Za-z-]{8,64}`），
/// 模型改写/编造的引用天然不匹配。
fn extract_chat_image_references(text: &str) -> Vec<String> {
    const PROTOCOL: &str = "chat-image://";
    let mut references = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(PROTOCOL) {
        let after = &rest[start + PROTOCOL.len()..];
        // 前缀全部为 ASCII 白名单字节，字节索引切片安全。
        let id_len = after
            .bytes()
            .position(|b| !(b.is_ascii_alphanumeric() || b == b'-'))
            .unwrap_or(after.len());
        let candidate = &after[..id_len];
        if (8..=64).contains(&candidate.len()) {
            references.push(candidate.to_string());
        }
        // 前进量至少一个字符（id 为空时跳过首个非白名单字符，UTF-8 安全）。
        let skip = if id_len > 0 {
            id_len
        } else {
            after.chars().next().map_or(after.len(), char::len_utf8)
        };
        rest = &after[skip..];
    }
    references
}

#[cfg(test)]
mod tests;

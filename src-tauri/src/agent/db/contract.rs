//! 会话持久化的 JSON 契约类型。
//!
//! 迁移自旧 `agent/llm.rs`：这些类型不是「LLM 客户端协议」，而是**落库形状**
//! （`dispatcher_messages.tool_calls_json` 与消息内容 parts、
//! `dispatcher_session_token_usage` 的用量字段）——模型调用已全面改用 rig
//! 契约，但库中既有数据必须继续以同一形状读写，故保留在此。
//!
//! `ChatMessage` 的 LLM 侧用途（构造请求）已由 rig `Message` 取代；现存职责是
//! 历史回放解析（`DispatcherMessageRecord` 记录 → 由 `rig_ext::message`
//! 转为 rig 消息）。

use serde::{Deserialize, Serialize};

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

/// 本轮工具图片附加上限：每次请求最多内联的最近引用数（防 base64 请求
/// 膨胀；更早的引用模型仍可用 analyze_image 按需查看）。
pub(crate) const MAX_TURN_TOOL_IMAGE_ATTACHMENTS: usize = 3;

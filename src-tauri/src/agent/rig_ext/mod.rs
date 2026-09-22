//! rig 适配层（rig-core 0.42 "portable contracts" 哲学：模型/消息/工具/流式
//! 契约由 rig 提供，运行时循环由本模块基于契约组合）。
//!
//! - `model`：用途槽位（chat/vision/summary）解析与模型工厂，
//!   `PurposeSwitchingModel`（按请求是否含图委托 chat/vision 模型）；
//! - `message`：`DispatcherMessageRecord` → rig `Message` 消息桥
//!   （含 `chat-image://` 图片段 → base64 `UserContent::Image`）；
//! - `tool_result`：工具结果落库 + 压缩管线（迁移自 `common::tool_result`）；
//! - `loop`（`r#loop`）：多轮工具运行时循环，消费 rig 流式事件 → `AgentEvent`。
//!
//! 类型约束：本模块的公开接口只出现 rig 契约类型与 app 领域类型。
//! 例外（落库边界）：`llm::OutboundToolCall` / `llm::LlmUsage` 是 DB 持久化
//! JSON 契约（`tool_calls_json` 列与 `dispatcher_session_token_usage` 表的
//! 落库形态，非 provider 类型），仅在写库前转换；Phase 5 删除 `llm/` 时
//! 随 DB 类型一起迁移归位。

// Phase 1 仅建设适配层，尚未接入任何运行路径；Phase 3 接入后移除该 allow。
#![allow(dead_code)]

pub(crate) mod r#loop;
pub(crate) mod message;
pub(crate) mod model;
pub(crate) mod review;
pub(crate) mod agents;
pub(crate) mod sub_agent;
pub(crate) mod summary;
pub(crate) mod tool_result;
pub(crate) mod tools;

use crate::agent::llm::{LlmPromptTokensDetails, LlmUsage};

/// rig `Usage` → 落库用 `LlmUsage`（`dispatcher_session_token_usage` 表的
/// 写入契约）。字段映射：input→prompt、output→completion、total 缺省（0）
/// 时按 input+output 补齐（对齐 `common::usage::normalized_total_tokens`）。
pub(crate) fn llm_usage_from_rig(usage: &rig::completion::Usage) -> LlmUsage {
    LlmUsage {
        prompt_tokens: usage.input_tokens,
        completion_tokens: usage.output_tokens,
        total_tokens: if usage.total_tokens > 0 {
            usage.total_tokens
        } else {
            usage.input_tokens + usage.output_tokens
        },
        prompt_tokens_details: (usage.cached_input_tokens > 0).then_some(LlmPromptTokensDetails {
            cached_tokens: usage.cached_input_tokens,
        }),
    }
}

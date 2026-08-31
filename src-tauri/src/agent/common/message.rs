use std::collections::HashMap;

use anyhow::Result;
use tauri::ipc::Channel;

use super::super::db::{DispatcherDb, DispatcherMessageRecord, DispatcherMessageUsageStats};
use super::super::llm::{
    messages_contain_images, ChatMessage, FunctionCall, OpenAiCompatProvider, OutboundToolCall,
    RequestedToolCall,
};
use super::super::run_loop::AgentEvent;
use super::super::tools::ToolRegistry;
use super::emit;

// ─── Assistant Message Persistence ───────────────────────────────────────────────

pub async fn persist_assistant_message(
    db: &DispatcherDb,
    workspace_id: &str,
    content: &str,
    usage_stats: &DispatcherMessageUsageStats,
) -> Result<DispatcherMessageRecord> {
    db.add_visible_message_with_usage_async(workspace_id, "assistant", content, usage_stats)
        .await
}

pub async fn persist_tool_calls_message(
    db: &DispatcherDb,
    workspace_id: &str,
    content: &str,
    tool_calls: &[OutboundToolCall],
    thinking_content: &str,
    thinking_elapsed_ms: Option<u64>,
) -> Result<DispatcherMessageRecord> {
    db.add_visible_message_with_tools_and_thinking_async(
        workspace_id,
        "assistant",
        content,
        None,
        None,
        None,
        Some(tool_calls),
        if thinking_content.is_empty() {
            None
        } else {
            Some(thinking_content)
        },
        thinking_elapsed_ms.unwrap_or(0),
    )
    .await
}

pub fn build_tool_calls_payload(
    tool_calls: &[RequestedToolCall],
    registry: &ToolRegistry,
) -> Result<Vec<OutboundToolCall>> {
    tool_calls
        .iter()
        .map(|call| {
            let enriched = registry.effective_args(&call.name, &call.arguments);
            let args_json = serialize_tool_arguments(&call.name, &enriched)?;
            Ok(OutboundToolCall {
                id: call.id.clone(),
                kind: "function".to_string(),
                function: FunctionCall {
                    name: call.name.clone(),
                    arguments: args_json,
                },
            })
        })
        .collect()
}

pub fn build_args_map(
    tool_calls: &[RequestedToolCall],
    registry: &ToolRegistry,
) -> Result<HashMap<String, String>> {
    tool_calls
        .iter()
        .map(|tc| {
            let enriched = registry.effective_args(&tc.name, &tc.arguments);
            let args_json = serialize_tool_arguments(&tc.name, &enriched)?;
            Ok((tc.id.clone(), args_json))
        })
        .collect()
}

/// 序列化工具参数供模型/前端展示。
///
/// G9-14：失败（如非有限浮点数）不再记日志降级为空对象 `{}`，而是返回错误上抛——
/// 静默降级会让模型/前端看到的参数与工具实际执行所用的 effective_args 不一致
/// 且无线索可查。调用方（run loop 的工具执行入口）以 `?` 透传，运行以 Failed
/// 事件收口；错误消息以「错误：」开头，符合前端展示与 `is_tool_error_message`
/// 的既有契约。实践中 LLM 响应经 JSON 解析得到的参数不可能含非有限浮点数，
/// 该分支是防御性兜底。
pub(crate) fn serialize_tool_arguments(
    tool_name: &str,
    arguments: &serde_json::Value,
) -> Result<String> {
    serde_json::to_string(arguments)
        .map_err(|error| anyhow::anyhow!("错误：工具 '{tool_name}' 参数序列化失败：{error}"))
}

// ─── Vision Model Selection ──────────────────────────────────────────────────────

/// Pick the provider for one run-loop iteration.
///
/// When the pending messages contain images, the pre-built `vision_provider`
/// (constructed from the configured vision model's own url/apiKey/model) is
/// used instead of the chat provider — the vision model may live on a
/// different gateway, so swapping only the model name is not enough.
pub fn select_provider_for_messages(
    provider: &OpenAiCompatProvider,
    messages: &[ChatMessage],
    vision_provider: Option<&OpenAiCompatProvider>,
    on_event: &Channel<AgentEvent>,
    notify_user: bool,
) -> Result<OpenAiCompatProvider> {
    if !messages_contain_images(messages) {
        return Ok(provider.clone());
    }

    let Some(vision) = vision_provider else {
        anyhow::bail!("检测到用户上传了图片，但视觉模型未配置。请先在设置中配置视觉模型后重试。");
    };

    let selected = vision.clone();
    // 视觉模型可能部署在独立 gateway（url/apiKey 不同）：仅模型名相同并不代表
    // 同一 provider。三项（模型名/网关/密钥）全部一致才视为未切换，否则即通知。
    let same_provider = selected.model() == provider.model()
        && selected.api_base() == provider.api_base()
        && selected.api_key() == provider.api_key();
    if notify_user && !same_provider {
        emit(
            on_event,
            AgentEvent::ModelSwitched {
                from_model: provider.model().to_string(),
                to_model: selected.model().to_string(),
                reason: "检测到用户上传了图片".to_string(),
            },
        );
    }

    Ok(selected)
}

// ─── LLM Context Filtering ─────────────────────────────────────────────────────

/// 纯调度 plumbing 工具名：其 assistant/tool 消息不进入 LLM 上下文。
/// 本常量是 LLM 上下文过滤的唯一口径来源；DB 加载路径
/// （`db::messages::load_llm_history`）直接委托 `should_keep_llm_message`。
const DISPATCH_PLUMBING_TOOL_NAMES: [&str; 6] = [
    "dispatch_claude",
    "dispatch_codex",
    "continue_claude_session",
    "continue_codex_session",
    "exit_claude_session",
    "exit_codex_session",
];

/// 消息是否应保留在 LLM 上下文中（全仓唯一实现，G9-05）。
///
/// 过滤纯调度 plumbing 工具（dispatch_claude 等）的工具结果，以及仅承载
/// 流程状态、对模型决策无意义的 process-only assistant 消息。
/// 内存追加路径（`AgentLoop::append`）与 DB 加载路径
/// （`db::messages::load_llm_history` / `DispatcherMessageRecord::to_llm_message`）
/// 均直接委托本函数，保证「同 run 多轮迭代」与「新 run 从 DB 重新加载」
/// 使用同一上下文口径，不再存在双份实现漂移的可能。
pub(crate) fn should_keep_llm_message(message: &ChatMessage) -> bool {
    match message.role.as_str() {
        "assistant" => {
            !is_process_only_assistant_message(&message.content)
                && !is_process_only_assistant_tool_call(message)
        }
        "tool" => !message
            .name
            .as_deref()
            .is_some_and(is_dispatch_plumbing_tool_name),
        _ => true,
    }
}

fn is_process_only_assistant_message(content: &str) -> bool {
    let trimmed = content.trim();
    matches!(
        trimmed,
        "🔄 子任务当前轮次已完成"
            | "✅ 子任务进程已结束"
            | "⚠️ 子任务进程已失败退出"
            | "⏹️ 子任务进程已取消"
            | "🔄 子任务当前轮次已完成，执行结果已同步供后续分析。"
            | "✅ 子任务进程已结束，执行结果已同步供后续分析。"
            | "⚠️ 子任务进程已失败退出，执行结果已同步供后续分析。"
            | "⏹️ 子任务进程已取消，执行结果已同步供后续分析。"
    ) || trimmed.starts_with("📋 已自动批准 ")
        || content.starts_with("📋 已提交 ")
        || content.starts_with("📨 已向 ")
        || content.starts_with("⏹️ 已向 ")
}

fn is_process_only_assistant_tool_call(message: &ChatMessage) -> bool {
    message
        .tool_calls
        .as_ref()
        .is_some_and(|calls| !calls.is_empty() && calls.iter().all(is_dispatch_plumbing_tool_call))
}

fn is_dispatch_plumbing_tool_call(call: &OutboundToolCall) -> bool {
    is_dispatch_plumbing_tool_name(&call.function.name)
}

fn is_dispatch_plumbing_tool_name(name: &str) -> bool {
    DISPATCH_PLUMBING_TOOL_NAMES.contains(&name)
}

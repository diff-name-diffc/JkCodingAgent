use anyhow::Result;

use super::super::db::{DispatcherDb, DispatcherMessageRecord, DispatcherMessageUsageStats};
use crate::agent::db::{ChatMessage, OutboundToolCall};

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

// ─── LLM Context Filtering ─────────────────────────────────────────────────────

/// 纯调度 plumbing 工具名：其 assistant/tool 消息不进入 LLM 上下文。
/// 本常量是 LLM 上下文过滤的唯一口径来源；DB 加载路径
/// （`db::messages::queries::load_llm_history`）直接委托 `should_keep_llm_message`。
/// 这些工具名只存在于老库的历史行（dispatch 子进程系统已下线），保留过滤
/// 是为了不让旧会话的 plumbing 消息重新灌进上下文。
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
/// DB 加载路径（`db::messages::queries::load_llm_history`）直接委托本函数，
/// 「新 run 从 DB 重新加载」因而不存在第二份同口径实现。
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

// ─── LLM Context Repair ────────────────────────────────────────────────────────

/// 未应答工具调用的占位结果文案（模型据此知道该结果不存在、可重新调用）。
/// rig `Message` 侧的整形兜底（`rig_ext::context::repair_pairing`）复用同一文案。
pub(crate) const UNANSWERED_TOOL_RESULT_PLACEHOLDER: &str =
    "（该工具调用没有产生结果：运行被中断，结果未落库。若仍需要，请重新调用。）";

/// 修复「assistant tool_calls ↔ tool 结果」配对（全仓唯一实现）。
///
/// 历史里可能缺少某个调用的结果行——run 在批量中途被取消、进程被杀、前序工具
/// 致命失败中止——也可能留下没有对应 assistant 的孤儿结果（其 assistant 被
/// `should_keep_llm_message` 过滤掉）。两者都会让服务端以 400 拒绝整轮请求
/// （assistant 的 tool_calls 之后必须跟齐 tool 消息），因此装配上下文前必须按
/// 调用顺序补齐缺失结果、剔除孤儿结果。写侧由运行循环保证成对落库（含取消/致命
/// 失败的占位补齐），此处只是读侧防御校验：触发即留痕——频繁出现说明写侧回归。
/// 库中既有的残缺历史无需数据迁移即可继续使用。
pub(crate) fn repair_tool_call_pairing(messages: &mut Vec<ChatMessage>) {
    let mut pending: Vec<ChatMessage> = std::mem::take(messages);
    pending.reverse();
    let mut repaired: Vec<ChatMessage> = Vec::with_capacity(pending.len());
    let mut placeholders_added = 0usize;
    let mut orphans_dropped = 0usize;

    while let Some(message) = pending.pop() {
        if message.role != "assistant" {
            // 排在 assistant 之外的工具结果是孤儿：没有前置 tool_calls 可应答。
            if message.role != "tool" {
                repaired.push(message);
            } else {
                orphans_dropped += 1;
            }
            continue;
        }

        let calls = message.tool_calls.clone().unwrap_or_default();
        if calls.is_empty() {
            repaired.push(message);
            continue;
        }

        let mut results: Vec<ChatMessage> = Vec::new();
        while pending.last().is_some_and(|next| next.role == "tool") {
            if let Some(result) = pending.pop() {
                results.push(result);
            }
        }

        repaired.push(message);
        for call in &calls {
            let matched = results
                .iter()
                .position(|result| result.tool_call_id.as_deref() == Some(call.id.as_str()));
            match matched {
                Some(index) => repaired.push(results.remove(index)),
                None => {
                    placeholders_added += 1;
                    repaired.push(unanswered_tool_result(&call.id, &call.function.name));
                }
            }
        }
        // 其余结果没有对应的 tool_call：一并丢弃，避免出现响应错位的工具消息。
        orphans_dropped += results.len();
    }

    if placeholders_added > 0 || orphans_dropped > 0 {
        eprintln!(
            "repair_tool_call_pairing 触发防御修复：补占位 {placeholders_added} 条、\
             剔除孤儿结果 {orphans_dropped} 条（库中残缺历史）。写侧已保证配对，\
             若新会话频繁出现此日志说明写侧回归"
        );
    }
    *messages = repaired;
}

/// 补齐用的占位工具结果：只承载「未产生结果」这一事实，不伪装成执行错误。
fn unanswered_tool_result(tool_call_id: &str, tool_name: &str) -> ChatMessage {
    ChatMessage {
        role: "tool".to_string(),
        content: UNANSWERED_TOOL_RESULT_PLACEHOLDER.to_string(),
        content_parts: Vec::new(),
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: Some(tool_call_id.to_string()),
        name: Some(tool_name.to_string()),
        source_id: None,
    }
}

//! 审查载荷组装：命令执行工具（exec / local_zsh / ssh_exec）与 broker / MCP
//! 的审查入口共用同一套组装逻辑，保证审查模型获得的上下文（意图、任务、
//! 执行者任务、对话上下文、命令历史）完整且一致。

use serde_json::Value;

use crate::agent::command_history;
use crate::agent::ssh_review::{CommandReviewPayload, CommandReviewTarget};

use super::context::ToolContext;

/// 组装交给审查模型的载荷。
///
/// - `intent`：优先取本次工具调用自声明的 `compress_intent`（模型对本次调用
///   目的的说明），缺失时回退会话标题——与历史行为一致。
/// - `executor_task`：仅在存在且与用户任务不同时送审（避免重复区块）。
/// - `conversation`：上下文构建期预渲染的最近对话。
/// - `command_history`：实时读取本会话命令台账（轮内追加即时可见）。
pub fn build_review_payload(
    context: &ToolContext,
    args: Option<&Value>,
    target: CommandReviewTarget,
    command: String,
    stdin: Option<String>,
) -> CommandReviewPayload {
    let intent = args
        .and_then(|args| args.get("compress_intent"))
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| context.session_title.clone());
    CommandReviewPayload {
        intent,
        task: context.user_task.clone().unwrap_or_default(),
        executor_task: context.executor_task.clone(),
        conversation: context.review_conversation.clone(),
        target,
        command_history: command_history::render_for_review(&context.workspace_id),
        command,
        stdin,
    }
}

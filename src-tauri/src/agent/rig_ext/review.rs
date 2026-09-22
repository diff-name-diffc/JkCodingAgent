//! 命令安全审查上下文（rig 形态的 `review_context`）。
//!
//! 旧实现经 `ToolContext` 取审查输入（`tools/review_context.rs`）；rig 工具
//! 层不再持有 ToolContext，输入改为构造期依赖 `RigReviewContext`（随
//! `RigToolDeps.review` 传入）。载荷字段与组装规则逐条对齐旧实现：意图取
//! 本次调用的 `compress_intent`（缺失回退会话标题）、执行者任务仅在存在时
//! 送审、对话上下文为预渲染文本、命令台账实时读取。

use serde_json::Value;

use crate::agent::command_history;
use crate::agent::db::settings::SshReviewConfig;
use crate::agent::ssh_review::{CommandReviewPayload, CommandReviewTarget};

/// 命令类工具的安全审查输入。`config = None` 表示未配置审查模型——
/// 命令类工具据此 fail-closed 拒绝执行（对齐旧 `ToolContext::ssh_review`）。
#[derive(Clone)]
pub struct RigReviewContext {
    pub config: Option<SshReviewConfig>,
    pub session_title: String,
    pub user_task: Option<String>,
    pub executor_task: Option<String>,
    /// 上下文构建期预渲染的最近对话（`ssh_review::render_dialogue_for_review`）。
    pub review_conversation: Option<String>,
}

impl RigReviewContext {
    /// 无审查配置的最小上下文（配置缺失但字段齐全，便于工具统一走
    /// 「未配置 → 拒绝」分支而不是 Option 链）。
    pub fn unconfigured() -> Self {
        Self {
            config: None,
            session_title: String::new(),
            user_task: None,
            executor_task: None,
            review_conversation: None,
        }
    }

    /// 组装交给审查模型的载荷（对齐旧 `build_review_payload`）。
    pub fn build_payload(
        &self,
        workspace_id: &str,
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
            .unwrap_or_else(|| self.session_title.clone());
        CommandReviewPayload {
            intent,
            task: self.user_task.clone().unwrap_or_default(),
            executor_task: self.executor_task.clone(),
            conversation: self.review_conversation.clone(),
            target,
            command_history: command_history::render_for_review(workspace_id),
            command,
            stdin,
        }
    }
}

impl std::fmt::Debug for RigReviewContext {
    /// 内含审查模型凭据：Debug 脱敏（与 `ToolContext` 同口径）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RigReviewContext")
            .field("configured", &self.config.is_some())
            .field("session_title", &self.session_title)
            .field("user_task", &self.user_task)
            .field("executor_task", &self.executor_task)
            .field(
                "review_conversation",
                &self.review_conversation.as_ref().map(|text| text.len()),
            )
            .finish()
    }
}

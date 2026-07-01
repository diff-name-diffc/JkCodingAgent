use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::Emitter;

use super::manager::SubAgentManager;
use super::runtime::{SubAgentEvent, SubAgentEventPayload, SubAgentRuntime};
use crate::agent::tools::{AgentTool, ToolContext};

pub const SUB_AGENT_FAILURE_PREFIX: &str = "__SUB_AGENT_FAILURE__:";

/// 用失败前缀包装消息。父循环（tool_exec.rs）会检测此前缀，
/// 一旦命中就把子智能体失败升级为整个父循环的致命错误——
/// 因为让主 Agent 基于一个不完整的委派结果继续推理是不可接受的。
pub fn sub_agent_failure(message: impl AsRef<str>) -> String {
    format!("{}{}", SUB_AGENT_FAILURE_PREFIX, message.as_ref())
}

/// 检测结果是否带失败前缀，命中则返回其后的错误说明。
/// 由父循环在 tool_exec.rs 中调用，决定是否升级为致命错误。
pub fn sub_agent_failure_message(result: &str) -> Option<&str> {
    result.strip_prefix(SUB_AGENT_FAILURE_PREFIX)
}

pub struct NotifyUserProgressTool;

pub fn notify_user_progress_tool() -> Box<dyn AgentTool> {
    Box::new(NotifyUserProgressTool)
}

/// LLM 工具 `call_sub_agent`：由主 Agent 调用以委派子任务。
/// 执行成功返回子智能体结果文本；失败则用 sub_agent_failure 前缀包装，
/// 父循环检测到前缀即升级为致命错误。
pub struct SubAgentTool {
    manager: Arc<SubAgentManager>,
}

impl SubAgentTool {
    pub fn new(manager: Arc<SubAgentManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AgentTool for NotifyUserProgressTool {
    fn name(&self) -> &'static str {
        "notify_user_progress"
    }

    fn description(&self) -> &'static str {
        "子智能体专用：向用户主动发送阶段性进度、当前发现、阻塞点或下一步计划。适合长时间任务中每完成一个有意义阶段调用一次，消息会直接展示在正文中。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "给用户看的简洁进度说明，应说明当前状态、已完成内容、正在做什么或遇到的阻塞。"
                }
            },
            "required": ["message"]
        })
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let message = args
            .get("message")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let Some(message) = message else {
            return sub_agent_failure("错误：message 参数不能为空");
        };

        let Some(agent_id) = context.current_sub_agent_id.as_deref() else {
            return sub_agent_failure("错误：notify_user_progress 只能由子智能体调用");
        };
        let agent_name = context
            .current_sub_agent_name
            .as_deref()
            .unwrap_or(agent_id)
            .to_string();
        let Some(app_handle) = &context.app_handle else {
            return sub_agent_failure("错误：无法发送子智能体进度通知，缺少 AppHandle");
        };

        let visible_message = trim_progress_message(message);
        if let Err(error) = app_handle.emit(
            "sub-agent-event",
            SubAgentEventPayload {
                session_id: context.workspace_id.clone(),
                event: SubAgentEvent::Progress {
                    agent_id: agent_id.to_string(),
                    agent_name,
                    message: visible_message.clone(),
                },
            },
        ) {
            return sub_agent_failure(format!("错误：发送进度通知失败：{error}"));
        }

        format!("已通知用户：{visible_message}")
    }
}

fn trim_progress_message(message: &str) -> String {
    const MAX_CHARS: usize = 2_000;
    let trimmed = message.trim();
    if trimmed.chars().count() <= MAX_CHARS {
        return trimmed.to_string();
    }
    format!("{}...", trimmed.chars().take(MAX_CHARS).collect::<String>())
}

#[async_trait]
impl AgentTool for SubAgentTool {
    fn name(&self) -> &'static str {
        "call_sub_agent"
    }

    fn description(&self) -> &'static str {
        "调用一个子智能体执行特定领域的复杂任务。子智能体拥有独立的执行上下文，内部的工具调用过程对你透明，你只会收到最终结果。可用子智能体列表通过 list_sub_agents 获取。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "string",
                    "description": "子智能体的 ID。通过 list_sub_agents 查看可用列表。"
                },
                "task": {
                    "type": "string",
                    "description": "要交给子智能体的任务描述，应清晰说明期望的行为和输出格式。"
                }
            },
            "required": ["agent_id", "task"]
        })
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let agent_id = args
            .get("agent_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let task = args
            .get("task")
            .and_then(|v| v.as_str())
            .unwrap_or_default();

        if agent_id.is_empty() {
            return sub_agent_failure("错误：agent_id 参数不能为空");
        }
        if task.is_empty() {
            return sub_agent_failure("错误：task 参数不能为空");
        }

        let config = match self.manager.get(agent_id) {
            Some(c) if c.enabled => c,
            Some(_) => return sub_agent_failure(format!("错误：子智能体 '{}' 已被禁用", agent_id)),
            None => return sub_agent_failure(format!("错误：未找到子智能体 '{}'", agent_id)),
        };

        let Some(parent_provider) = &context.llm_provider else {
            return sub_agent_failure("错误：无法获取主 Agent 的 LLM Provider 配置");
        };

        let Some(parent_tools) = context.sub_agent_tool_registry.as_ref() else {
            return sub_agent_failure("错误：工具注册表不支持子智能体调用");
        };

        let app_handle = context.app_handle.clone();
        let session_id = context.workspace_id.clone();

        match SubAgentRuntime::build(
            &config,
            parent_provider,
            Arc::clone(parent_tools),
            context.clone(),
        ) {
            Ok(runtime) => match runtime.execute(task, app_handle, &session_id).await {
                Ok(result) => result,
                Err(e) => sub_agent_failure(format!("子智能体执行失败：{}", e)),
            },
            Err(e) => sub_agent_failure(format!("子智能体初始化失败：{}", e)),
        }
    }
}

pub struct ListSubAgentsTool {
    manager: Arc<SubAgentManager>,
}

impl ListSubAgentsTool {
    pub fn new(manager: Arc<SubAgentManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AgentTool for ListSubAgentsTool {
    fn name(&self) -> &'static str {
        "list_sub_agents"
    }

    fn description(&self) -> &'static str {
        "列出当前可用的全部子智能体及其描述，帮助你决定调用哪个子智能体。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, _args: &Value, context: &ToolContext) -> String {
        let configs = match self.manager.get_enabled_for_session(&context.workspace_id) {
            Ok(c) => c,
            Err(e) => return format!("错误：无法获取子智能体列表：{}", e),
        };

        if configs.is_empty() {
            return "当前会话没有可用的子智能体。请在设置中配置并启用子智能体。".to_string();
        }

        let mut output = String::from("可用的子智能体列表：\n\n");
        for config in &configs {
            output.push_str(&format!(
                "### {} ({})\n{}\n\n",
                config.agent_name, config.agent_id, config.description
            ));
        }
        output.push_str("使用 call_sub_agent(agent_id=\"...\", task=\"...\") 来调用子智能体。");
        output
    }
}

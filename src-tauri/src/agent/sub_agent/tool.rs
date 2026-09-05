use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::Emitter;

use super::manager::SubAgentManager;
use super::runtime::{record_trace_event, SubAgentEvent, SubAgentEventPayload, SubAgentRuntime};
use crate::agent::tools::{AgentTool, ToolContext, ToolResult};

pub struct NotifyUserProgressTool;

pub fn notify_user_progress_tool() -> Box<dyn AgentTool> {
    Box::new(NotifyUserProgressTool)
}

/// LLM 工具 `call_sub_agent`：由主 Agent 调用以委派子任务。
/// 执行成功返回子智能体结果文本；失败直接返回 FatalError，父循环不得基于
/// 不完整的委派结果继续推理。
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

    async fn execute(&self, args: &Value, context: &ToolContext) -> ToolResult {
        let message = args
            .get("message")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let Some(message) = message else {
            return ToolResult::fatal_error("错误：message 参数不能为空");
        };

        let Some(agent_id) = context.current_sub_agent_id.as_deref() else {
            return ToolResult::fatal_error("错误：notify_user_progress 只能由子智能体调用");
        };
        let agent_name = context
            .current_sub_agent_name
            .as_deref()
            .unwrap_or(agent_id)
            .to_string();
        let Some(app_handle) = &context.app_handle else {
            return ToolResult::fatal_error("错误：无法发送子智能体进度通知，缺少 AppHandle");
        };

        let visible_message = trim_progress_message(message);
        let Some(tool_call_id) = context.sub_agent_parent_tool_call_id.as_deref() else {
            return ToolResult::fatal_error("错误：子智能体进度通知缺少父级 tool_call_id");
        };
        let event = SubAgentEvent::Progress {
            agent_id: agent_id.to_string(),
            agent_name,
            message: visible_message.clone(),
        };
        let timestamp_ms = chrono::Utc::now().timestamp_millis();
        if let Some(trace) = &context.sub_agent_trace_events {
            if let Ok(value) = serde_json::to_value(&event) {
                record_trace_event(trace, value, timestamp_ms);
            }
        }
        if let Err(error) = app_handle.emit(
            "sub-agent-event",
            SubAgentEventPayload {
                session_id: context.workspace_id.clone(),
                tool_call_id: tool_call_id.to_string(),
                timestamp_ms,
                event,
            },
        ) {
            return ToolResult::fatal_error(format!("错误：发送进度通知失败：{error}"));
        }

        ToolResult::success_data(
            json!({ "notified": true, "message": visible_message }),
            format!("已通知用户：{visible_message}"),
            format!("已通知用户：{visible_message}"),
        )
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

    async fn execute(&self, args: &Value, context: &ToolContext) -> ToolResult {
        // LLM 传参常带前后空白/换行：先 trim 再校验，避免纯空白参数
        // 绕过空值检查、白白触发一次子智能体调用（G13-02）。
        let agent_id = args
            .get("agent_id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or_default();
        let task = args
            .get("task")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or_default();

        if agent_id.is_empty() {
            return ToolResult::fatal_error("错误：agent_id 参数不能为空");
        }
        if task.is_empty() {
            return ToolResult::fatal_error("错误：task 参数不能为空");
        }

        let config = match self.manager.get(agent_id) {
            Some(c) if c.enabled => c,
            Some(_) => {
                return ToolResult::fatal_error(format!("错误：子智能体 '{}' 已被禁用", agent_id))
            }
            None => return ToolResult::fatal_error(format!("错误：未找到子智能体 '{}'", agent_id)),
        };

        let Some(parent_provider) = &context.llm_provider else {
            return ToolResult::fatal_error("错误：无法获取主 Agent 的 LLM Provider 配置");
        };

        let Some(parent_tools) = context.sub_agent_tool_registry.as_ref() else {
            return ToolResult::fatal_error("错误：工具注册表不支持子智能体调用");
        };

        let app_handle = context.app_handle.clone();
        let session_id = context.workspace_id.clone();

        let Some(tool_call_id) = context.current_tool_call_id.clone() else {
            return ToolResult::fatal_error("错误：调用子智能体缺少 tool_call_id");
        };

        match SubAgentRuntime::build(
            &config,
            parent_provider,
            Arc::clone(parent_tools),
            context.clone(),
        ) {
            Ok(mut runtime) => {
                let outcome = runtime.execute(task, app_handle, &session_id).await;
                let trace_json = match runtime.trace_events_json() {
                    Ok(trace) => trace,
                    Err(error) => {
                        return ToolResult::fatal_error(format!(
                            "错误：子智能体轨迹序列化失败：{error}"
                        ));
                    }
                };
                let status = if outcome.is_ok() {
                    "completed"
                } else {
                    "failed"
                };
                let manager = Arc::clone(&self.manager);
                let persist_workspace_id = session_id.clone();
                let persist_tool_call_id = tool_call_id.clone();
                let persist_agent_id = config.agent_id.clone();
                let persist_status = status.to_string();
                let persisted = tokio::task::spawn_blocking(move || {
                    manager.save_run_trace(
                        &persist_workspace_id,
                        &persist_tool_call_id,
                        &persist_agent_id,
                        &persist_status,
                        &trace_json,
                    )
                })
                .await;
                match persisted {
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => {
                        return ToolResult::fatal_error(format!(
                            "错误：子智能体轨迹持久化失败：{error}"
                        ));
                    }
                    Err(error) => {
                        return ToolResult::fatal_error(format!(
                            "错误：子智能体轨迹任务失败：{error}"
                        ));
                    }
                }
                match outcome {
                    Ok(result) => ToolResult::success_text(result),
                    Err(error) => {
                        ToolResult::fatal_error(format!("错误：子智能体执行失败：{error}"))
                    }
                }
            }
            Err(e) => ToolResult::fatal_error(format!("错误：子智能体初始化失败：{}", e)),
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

    async fn execute(&self, _args: &Value, context: &ToolContext) -> ToolResult {
        // get_enabled_for_session 内部是同步 SQLite 读取：阻塞 I/O 必须
        // 走 spawn_blocking，不得阻塞 async 运行时（项目规范）。
        let manager = Arc::clone(&self.manager);
        let workspace_id = context.workspace_id.clone();
        let joined =
            tokio::task::spawn_blocking(move || manager.get_enabled_for_session(&workspace_id))
                .await;
        let configs = match joined {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => {
                return ToolResult::recoverable_error(format!("错误：无法获取子智能体列表：{}", e))
            }
            Err(e) => {
                return ToolResult::recoverable_error(format!(
                    "错误：子智能体列表查询任务失败：{}",
                    e
                ))
            }
        };

        if configs.is_empty() {
            return ToolResult::success_data(
                json!({ "agents": [] }),
                "当前会话没有可用的子智能体。请在设置中配置并启用子智能体。",
                "当前会话没有可用的子智能体。请在设置中配置并启用子智能体。",
            );
        }

        let mut output = String::from("可用的子智能体列表：\n\n");
        for config in &configs {
            output.push_str(&format!(
                "### {} ({})\n{}\n\n",
                config.agent_name, config.agent_id, config.description
            ));
        }
        output.push_str("使用 call_sub_agent(agent_id=\"...\", task=\"...\") 来调用子智能体。");
        let data = json!({
            "agents": configs.iter().map(|config| json!({
                "id": config.agent_id,
                "name": config.agent_name,
                "description": config.description,
            })).collect::<Vec<_>>()
        });
        ToolResult::success_data(data, output.clone(), output)
    }
}

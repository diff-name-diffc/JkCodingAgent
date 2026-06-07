use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::manager::SubAgentManager;
use super::runtime::SubAgentRuntime;
use crate::agent::tools::{AgentTool, ToolContext};

pub struct SubAgentTool {
    manager: Arc<SubAgentManager>,
}

impl SubAgentTool {
    pub fn new(manager: Arc<SubAgentManager>) -> Self {
        Self { manager }
    }
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
            return "错误：agent_id 参数不能为空".to_string();
        }
        if task.is_empty() {
            return "错误：task 参数不能为空".to_string();
        }

        let config = match self.manager.get(agent_id) {
            Some(c) if c.enabled => c,
            Some(_) => return format!("错误：子智能体 '{}' 已被禁用", agent_id),
            None => return format!("错误：未找到子智能体 '{}'", agent_id),
        };

        let Some(parent_provider) = &context.llm_provider else {
            return "错误：无法获取主 Agent 的 LLM Provider 配置".to_string();
        };

        let Some(parent_tools) = context.sub_agent_tool_registry.as_ref() else {
            return "错误：工具注册表不支持子智能体调用".to_string();
        };

        let app_handle = context.app_handle.clone();
        let session_id = context.workspace_id.clone();

        match SubAgentRuntime::build(&config, parent_provider, Arc::clone(parent_tools), context.clone()) {
            Ok(runtime) => match runtime.execute(task, app_handle, &session_id).await {
                Ok(result) => result,
                Err(e) => format!("子智能体执行失败：{}", e),
            },
            Err(e) => format!("子智能体初始化失败：{}", e),
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
        output.push_str(
            "使用 call_sub_agent(agent_id=\"...\", task=\"...\") 来调用子智能体。",
        );
        output
    }
}

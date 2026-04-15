use async_trait::async_trait;
use serde_json::Value;

use super::context::ToolContext;
use crate::agent::llm::{ToolDefinition, ToolFunctionDefinition};
use crate::shared::truncate_for_display;

#[async_trait]
pub(crate) trait AgentTool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters(&self) -> Value;
    async fn execute(&self, args: &Value, context: &ToolContext) -> String;
}

pub struct ToolRegistry {
    tools: Vec<Box<dyn AgentTool>>,
}

impl ToolRegistry {
    pub(crate) fn new(tools: Vec<Box<dyn AgentTool>>) -> Self {
        Self { tools }
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|tool| ToolDefinition {
                kind: "function".to_string(),
                function: ToolFunctionDefinition {
                    name: tool.name().to_string(),
                    description: tool.description().to_string(),
                    parameters: tool.parameters(),
                },
            })
            .collect()
    }

    pub async fn execute(&self, name: &str, args: &Value, context: &ToolContext) -> String {
        match self.tools.iter().find(|tool| tool.name() == name) {
            Some(tool) => {
                let output = tool.execute(args, context).await;
                truncate_for_display(&output, context.max_result_chars, "\n\n[输出已截断]")
            }
            None => format!("错误：未找到工具 '{name}'"),
        }
    }
}

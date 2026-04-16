use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashSet;

use super::context::ToolContext;
use crate::agent::llm::{ToolDefinition, ToolFunctionDefinition};

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

    pub fn definitions_for_names<'a, I>(&self, allowed: Option<I>) -> Vec<ToolDefinition>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let allowed = allowed.map(|names| names.into_iter().collect::<HashSet<_>>());
        self.tools
            .iter()
            .filter(|tool| {
                allowed
                    .as_ref()
                    .map(|names| names.contains(tool.name()))
                    .unwrap_or(true)
            })
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
            Some(tool) => tool.execute(args, context).await,
            None => format!("错误：未找到工具 '{name}'"),
        }
    }
}

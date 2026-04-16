use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use super::context::ToolContext;
use crate::agent::llm::{ToolDefinition, ToolFunctionDefinition};

#[async_trait]
pub(crate) trait AgentTool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters(&self) -> Value;
    async fn execute(&self, args: &Value, context: &ToolContext) -> String;
}

#[async_trait]
pub(crate) trait DynamicToolProvider: Send + Sync {
    fn definitions_for_workspace(&self, workspace: &Path) -> Vec<ToolDefinition>;
    async fn execute(&self, name: &str, args: &Value, context: &ToolContext) -> Option<String>;
}

pub struct ToolRegistry {
    tools: Vec<Box<dyn AgentTool>>,
    dynamic_provider: Option<Arc<dyn DynamicToolProvider>>,
}

impl ToolRegistry {
    pub(crate) fn new(tools: Vec<Box<dyn AgentTool>>) -> Self {
        Self {
            tools,
            dynamic_provider: None,
        }
    }

    pub(crate) fn with_dynamic_provider(
        mut self,
        dynamic_provider: Arc<dyn DynamicToolProvider>,
    ) -> Self {
        self.dynamic_provider = Some(dynamic_provider);
        self
    }

    pub fn definitions_for_workspace<'a, I>(
        &self,
        workspace: &Path,
        allowed: Option<I>,
    ) -> Vec<ToolDefinition>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let allowed = allowed.map(|names| names.into_iter().collect::<HashSet<_>>());
        let static_definitions = self
            .tools
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
            .collect::<Vec<_>>();

        let mut definitions = static_definitions;
        if let Some(provider) = &self.dynamic_provider {
            definitions.extend(provider.definitions_for_workspace(workspace));
        }
        definitions
    }

    pub async fn execute(&self, name: &str, args: &Value, context: &ToolContext) -> String {
        match self.tools.iter().find(|tool| tool.name() == name) {
            Some(tool) => tool.execute(args, context).await,
            None => {
                if let Some(provider) = &self.dynamic_provider {
                    if let Some(result) = provider.execute(name, args, context).await {
                        return result;
                    }
                }
                format!("错误：未找到工具 '{name}'")
            }
        }
    }
}

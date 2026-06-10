use async_trait::async_trait;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use super::context::ToolContext;
use crate::agent::llm::{ToolDefinition, ToolFunctionDefinition};

#[async_trait]
pub trait AgentTool: Send + Sync {
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
    name_index: HashMap<String, usize>,
    dynamic_provider: Option<Arc<dyn DynamicToolProvider>>,
}

impl ToolRegistry {
    pub(crate) fn new(tools: Vec<Box<dyn AgentTool>>) -> Self {
        let name_index = tools
            .iter()
            .enumerate()
            .map(|(i, t)| (t.name().to_string(), i))
            .collect();
        Self {
            tools,
            name_index,
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

    pub fn add_tool(&mut self, tool: Box<dyn AgentTool>) {
        self.name_index
            .insert(tool.name().to_string(), self.tools.len());
        self.tools.push(tool);
    }

    fn find_by_name(&self, name: &str) -> Option<&Box<dyn AgentTool>> {
        self.name_index
            .get(name)
            .and_then(|&idx| self.tools.get(idx))
    }

    pub fn tool_names_and_descriptions(&self) -> Vec<(String, String)> {
        self.tools
            .iter()
            .map(|t| (t.name().to_string(), t.description().to_string()))
            .collect()
    }

    pub fn definitions_for_workspace<'a, I>(
        &self,
        workspace: &Path,
        allowed: Option<I>,
        include_dynamic: bool,
    ) -> Vec<ToolDefinition>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let allowed = allowed.map(|names| names.into_iter().collect::<HashSet<_>>());
        let mut definitions: Vec<ToolDefinition> = self
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
            .collect();

        if include_dynamic {
            if let Some(provider) = &self.dynamic_provider {
                definitions.extend(provider.definitions_for_workspace(workspace));
            }
        }
        definitions
    }

    pub async fn execute(&self, name: &str, args: &Value, context: &ToolContext) -> String {
        match self.find_by_name(name) {
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

    pub fn effective_args(&self, tool_name: &str, args: &Value) -> Value {
        let Some(tool) = self.find_by_name(tool_name) else {
            return args.clone();
        };
        let schema = tool.parameters();
        let Some(properties) = schema.get("properties").and_then(|v| v.as_object()) else {
            return args.clone();
        };

        let mut result = args.clone();
        let obj = match result.as_object_mut() {
            Some(obj) => obj,
            None => return args.clone(),
        };

        for (key, prop_schema) in properties {
            if !obj.contains_key(key) {
                if let Some(default_val) = prop_schema.get("default") {
                    obj.insert(key.clone(), default_val.clone());
                }
            }
        }
        result
    }
}

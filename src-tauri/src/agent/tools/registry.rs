use async_trait::async_trait;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use super::context::ToolContext;
use super::provider::ToolProvider;
use super::result::{ToolAction, ToolResult};
use super::spec::ToolSpec;
use super::ToolInput;
use crate::agent::llm::ToolDefinition;

#[async_trait]
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters(&self) -> Value;
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(self.name(), self.description(), self.parameters())
    }
    async fn execute(&self, args: &Value, context: &ToolContext) -> String;
}

#[async_trait]
pub(crate) trait DynamicToolProvider: Send + Sync {
    fn definitions_for_workspace(&self, workspace: &Path) -> Vec<ToolDefinition>;
    fn specs_for_workspace(&self, workspace: &Path) -> Vec<ToolSpec> {
        self.definitions_for_workspace(workspace)
            .into_iter()
            .map(|definition| {
                ToolSpec::mcp(
                    definition.function.name,
                    definition.function.description,
                    definition.function.parameters,
                )
            })
            .collect()
    }
    async fn execute(&self, name: &str, args: &Value, context: &ToolContext) -> Option<String>;
}

pub struct ToolRegistry {
    tools: Vec<Box<dyn AgentTool>>,
    name_index: HashMap<String, usize>,
    dynamic_provider: Option<Arc<dyn DynamicToolProvider>>,
}

impl ToolRegistry {
    pub(crate) fn new(tools: Vec<Box<dyn AgentTool>>) -> Self {
        let mut name_index = HashMap::new();
        for (index, tool) in tools.iter().enumerate() {
            let duplicate = name_index.insert(tool.name().to_string(), index);
            assert!(
                duplicate.is_none(),
                "duplicate agent tool registered: {}",
                tool.name()
            );
        }
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
        let duplicate = self
            .name_index
            .insert(tool.name().to_string(), self.tools.len());
        assert!(
            duplicate.is_none(),
            "duplicate agent tool registered: {}",
            tool.name()
        );
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
        if allowed.is_none() && include_dynamic {
            let provider: &dyn ToolProvider = self;
            let _provider_name = provider.provider_name();
            return provider
                .specs_for_workspace(workspace)
                .into_iter()
                .map(|spec| spec.to_definition())
                .collect();
        }

        self.specs_for_workspace(workspace, allowed, include_dynamic)
            .into_iter()
            .map(|spec| spec.to_definition())
            .collect()
    }

    pub fn specs_for_workspace<'a, I>(
        &self,
        workspace: &Path,
        allowed: Option<I>,
        include_dynamic: bool,
    ) -> Vec<ToolSpec>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let allowed = allowed.map(|names| names.into_iter().collect::<HashSet<_>>());
        let mut specs = self
            .tools
            .iter()
            .filter(|tool| {
                allowed
                    .as_ref()
                    .map(|names| names.contains(tool.name()))
                    .unwrap_or(true)
            })
            .map(|tool| tool.spec())
            .collect::<Vec<_>>();

        if include_dynamic {
            if let Some(provider) = &self.dynamic_provider {
                specs.extend(provider.specs_for_workspace(workspace));
            }
        }
        specs
    }

    pub fn spec_by_name(
        &self,
        workspace: &Path,
        name: &str,
        include_dynamic: bool,
    ) -> Option<ToolSpec> {
        if let Some(tool) = self.find_by_name(name) {
            return Some(tool.spec());
        }
        if include_dynamic {
            if let Some(provider) = &self.dynamic_provider {
                return provider
                    .specs_for_workspace(workspace)
                    .into_iter()
                    .find(|spec| spec.name == name);
            }
        }
        None
    }

    pub fn is_parallel_readonly(
        &self,
        workspace: &Path,
        name: &str,
        include_dynamic: bool,
    ) -> bool {
        self.spec_by_name(workspace, name, include_dynamic)
            .is_some_and(|spec| spec.supports_parallel_readonly())
    }

    pub async fn execute(&self, name: &str, args: &Value, context: &ToolContext) -> ToolResult {
        let input = ToolInput {
            call_id: String::new(),
            name: name.to_string(),
            arguments: args.clone(),
            effective_arguments: self.effective_args(name, args),
        };
        <Self as ToolProvider>::execute(self, name, input, context)
            .await
            .unwrap_or_else(|| ToolResult::recoverable_error(format!("错误：未找到工具 '{name}'")))
    }

    pub async fn execute_input(&self, input: ToolInput, context: &ToolContext) -> ToolResult {
        let ToolInput {
            call_id,
            name,
            arguments,
            effective_arguments,
        } = input;
        let _execution_metadata = (call_id, effective_arguments);

        match self.find_by_name(&name) {
            Some(tool) => attach_structured_action(
                &name,
                &arguments,
                ToolResult::from_text(tool.execute(&arguments, context).await)
                    .with_artifacts(Vec::new()),
            ),
            None => {
                if let Some(provider) = &self.dynamic_provider {
                    if let Some(result) = provider.execute(&name, &arguments, context).await {
                        return attach_structured_action(
                            &name,
                            &arguments,
                            ToolResult::from_text(result).with_artifacts(Vec::new()),
                        );
                    }
                }
                ToolResult::recoverable_error(format!("错误：未找到工具 '{}'", name))
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

#[async_trait]
impl ToolProvider for ToolRegistry {
    fn provider_name(&self) -> &'static str {
        "registry"
    }

    fn specs_for_workspace(&self, workspace: &Path) -> Vec<ToolSpec> {
        ToolRegistry::specs_for_workspace(
            self,
            workspace,
            Option::<std::iter::Empty<&str>>::None,
            true,
        )
    }

    async fn execute(
        &self,
        _name: &str,
        input: ToolInput,
        context: &ToolContext,
    ) -> Option<ToolResult> {
        Some(self.execute_input(input, context).await)
    }
}

fn attach_structured_action(name: &str, args: &Value, result: ToolResult) -> ToolResult {
    let Some(action) = structured_action_from_args(name, args) else {
        return result;
    };
    result.with_action(action)
}

fn structured_action_from_args(name: &str, args: &Value) -> Option<ToolAction> {
    match name {
        "message" => {
            args.get("content")
                .and_then(Value::as_str)
                .map(|content| ToolAction::FinalMessage {
                    content: content.to_string(),
                })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use std::path::Path;

    use super::{AgentTool, ToolRegistry};
    use crate::agent::tools::ToolContext;

    struct TestTool {
        name: &'static str,
    }

    #[async_trait]
    impl AgentTool for TestTool {
        fn name(&self) -> &'static str {
            self.name
        }

        fn description(&self) -> &'static str {
            "test tool"
        }

        fn parameters(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "flag": { "type": "boolean", "default": true }
                }
            })
        }

        async fn execute(&self, _args: &Value, _context: &ToolContext) -> String {
            "ok".to_string()
        }
    }

    #[test]
    #[should_panic(expected = "duplicate agent tool registered")]
    fn duplicate_tool_names_fail_loudly() {
        let _ = ToolRegistry::new(vec![
            Box::new(TestTool { name: "read_file" }),
            Box::new(TestTool { name: "read_file" }),
        ]);
    }

    #[test]
    fn spec_lookup_drives_parallel_readonly_policy() {
        let registry = ToolRegistry::new(vec![Box::new(TestTool { name: "read_file" })]);

        assert!(registry.is_parallel_readonly(Path::new("."), "read_file", false));
        assert!(!registry.is_parallel_readonly(Path::new("."), "missing", false));
    }

    #[test]
    fn effective_args_applies_schema_defaults() {
        let registry = ToolRegistry::new(vec![Box::new(TestTool { name: "read_file" })]);

        let defaulted = registry.effective_args("read_file", &json!({}));
        let explicit_false = registry.effective_args("read_file", &json!({ "flag": false }));

        assert_eq!(defaulted["flag"], true);
        assert_eq!(explicit_false["flag"], false);
    }
}

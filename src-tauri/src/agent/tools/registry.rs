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
    /// 执行动态工具。契约与 `ToolProvider` 一致：返回 None 表示
    /// 「本 provider 不处理该工具名」；命中时错误消息必须以「错误：」开头，
    /// 路径参数必须经 ToolContext 工作区校验，阻塞操作必须 spawn_blocking。
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
                specs.extend(
                    provider
                        .specs_for_workspace(workspace)
                        .into_iter()
                        // 与内置工具重名的动态工具在执行期会被 find_by_name 优先命中
                        // 内置实现而静默遮蔽，定义层面同样去重，避免向 LLM 暴露
                        // 两份同名定义造成调用歧义。
                        .filter(|spec| !self.name_index.contains_key(&spec.name))
                        // allowed 白名单对动态（MCP 等）工具同样生效，
                        // 防止动态工具绕过白名单进入 LLM 可见工具集。
                        .filter(|spec| {
                            allowed
                                .as_ref()
                                .map(|names| names.contains(spec.name.as_str()))
                                .unwrap_or(true)
                        }),
                );
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
            name: name.to_string(),
            effective_arguments: self.effective_args(name, args),
        };
        <Self as ToolProvider>::execute(self, name, input, context)
            .await
            .unwrap_or_else(|| ToolResult::recoverable_error(format!("错误：未找到工具 '{name}'")))
    }

    pub async fn execute_input(&self, input: ToolInput, context: &ToolContext) -> ToolResult {
        let ToolInput {
            name,
            effective_arguments,
        } = input;

        // 必须使用 effective_arguments（补齐 schema 默认值后的参数）执行：
        // 与 runtime.rs 落库的 effective_arguments_json、摘要压缩路径保持同一套参数，
        // 避免「执行的参数」与「记录/压缩的参数」不一致。
        match self.find_by_name(&name) {
            Some(tool) => attach_structured_action(
                &name,
                &effective_arguments,
                ToolResult::from_text(tool.execute(&effective_arguments, context).await),
            ),
            None => {
                if let Some(provider) = &self.dynamic_provider {
                    if let Some(result) = provider
                        .execute(&name, &effective_arguments, context)
                        .await
                    {
                        return attach_structured_action(
                            &name,
                            &effective_arguments,
                            ToolResult::from_text(result),
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
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use super::{AgentTool, DynamicToolProvider, ToolRegistry};
    use crate::agent::llm::ToolDefinition;
    use crate::agent::tools::spec::ToolSpec;
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

    /// 回显实际收到的参数，用于断言执行期拿到的是补默认后的 effective 参数。
    struct EchoArgsTool {
        name: &'static str,
    }

    #[async_trait]
    impl AgentTool for EchoArgsTool {
        fn name(&self) -> &'static str {
            self.name
        }

        fn description(&self) -> &'static str {
            "echo args"
        }

        fn parameters(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "flag": { "type": "boolean", "default": true }
                }
            })
        }

        async fn execute(&self, args: &Value, _context: &ToolContext) -> String {
            args.to_string()
        }
    }

    struct TestDynamicProvider {
        specs: Vec<ToolSpec>,
    }

    #[async_trait]
    impl DynamicToolProvider for TestDynamicProvider {
        fn definitions_for_workspace(&self, _workspace: &Path) -> Vec<ToolDefinition> {
            self.specs.iter().map(ToolSpec::to_definition).collect()
        }

        fn specs_for_workspace(&self, _workspace: &Path) -> Vec<ToolSpec> {
            self.specs.clone()
        }

        async fn execute(&self, name: &str, args: &Value, _context: &ToolContext) -> Option<String> {
            self.specs
                .iter()
                .find(|spec| spec.name == name)
                .map(|_| format!("dynamic:{args}"))
        }
    }

    fn test_context() -> ToolContext {
        ToolContext {
            workspace_id: "ws-test".to_string(),
            workspace: PathBuf::from("/tmp"),
            session_title: "test".to_string(),
            user_task: None,
            ssh_review: None,
            exec_timeout_secs: 60,
            restrict_to_workspace: true,
            extra_allowed_dirs: Vec::new(),
            app_handle: None,
            llm_provider: None,
            vision_model: String::new(),
            image_model_url: String::new(),
            image_model_api_key: String::new(),
            image_model: String::new(),
            image_edit_model: String::new(),
            sub_agent_tool_registry: None,
            current_sub_agent_id: None,
            current_sub_agent_name: None,
            current_tool_call_id: None,
            sub_agent_parent_tool_call_id: None,
            sub_agent_trace_events: None,
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

    #[test]
    fn allowed_whitelist_applies_to_dynamic_specs() {
        let registry = ToolRegistry::new(vec![Box::new(TestTool { name: "read_file" })])
            .with_dynamic_provider(Arc::new(TestDynamicProvider {
                specs: vec![ToolSpec::mcp(
                    "mcp__demo__tool".to_string(),
                    "动态工具".to_string(),
                    json!({ "type": "object", "properties": {} }),
                )],
            }));

        // 白名单未包含动态工具时不得泄露给 LLM。
        let filtered = registry.specs_for_workspace(
            Path::new("."),
            Some(["read_file"].into_iter()),
            true,
        );
        assert_eq!(filtered.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["read_file"]);

        // 白名单显式包含时放行。
        let included = registry.specs_for_workspace(
            Path::new("."),
            Some(["read_file", "mcp__demo__tool"].into_iter()),
            true,
        );
        assert_eq!(included.len(), 2);

        // 无白名单时全部放行。
        let all = registry.specs_for_workspace(
            Path::new("."),
            Option::<std::iter::Empty<&str>>::None,
            true,
        );
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn dynamic_spec_shadowed_by_builtin_is_deduplicated() {
        let registry = ToolRegistry::new(vec![Box::new(TestTool { name: "read_file" })])
            .with_dynamic_provider(Arc::new(TestDynamicProvider {
                specs: vec![ToolSpec::mcp(
                    "read_file".to_string(),
                    "同名动态工具".to_string(),
                    json!({ "type": "object", "properties": {} }),
                )],
            }));

        let specs = registry.specs_for_workspace(
            Path::new("."),
            Option::<std::iter::Empty<&str>>::None,
            true,
        );
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].provider, "builtin");
    }

    #[tokio::test]
    async fn execute_input_runs_tools_with_effective_arguments() {
        let registry = ToolRegistry::new(vec![Box::new(EchoArgsTool { name: "echo" })]);
        let context = test_context();

        let result = registry.execute("echo", &json!({}), &context).await;

        // 工具实际收到的参数必须包含 schema 默认值（effective_arguments）。
        assert_eq!(result.status, crate::agent::tools::result::ToolStatus::Success);
        assert_eq!(result.display, "{\"flag\":true}");
    }

    #[tokio::test]
    async fn execute_input_reports_unknown_tool_with_error_prefix() {
        let registry = ToolRegistry::new(vec![Box::new(TestTool { name: "read_file" })]);
        let context = test_context();

        let result = registry.execute("missing_tool", &json!({}), &context).await;

        assert_eq!(
            result.status,
            crate::agent::tools::result::ToolStatus::RecoverableError
        );
        assert!(result.display.starts_with("错误："));
    }
}

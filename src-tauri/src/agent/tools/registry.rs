use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::Mutex;

use super::context::ToolContext;
use super::result::ToolResult;
use super::spec::ToolSpec;
use super::ToolInput;
use crate::agent::llm::ToolDefinition;
use crate::mcp::McpScope;

#[async_trait]
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters(&self) -> Value;
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(self.name(), self.description(), self.parameters())
    }
    async fn execute(&self, args: &Value, context: &ToolContext) -> ToolResult;
}

#[async_trait]
pub(crate) trait DynamicToolProvider: Send + Sync {
    fn definitions_for_scope(&self, scope: &McpScope) -> Vec<ToolDefinition>;
    fn specs_for_scope(&self, scope: &McpScope) -> Vec<ToolSpec> {
        self.definitions_for_scope(scope)
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
    /// 执行动态工具。返回 None 表示
    /// 「本 provider 不处理该工具名」；命中时必须返回明确的 ToolResult，
    /// 路径参数必须经 ToolContext 工作区校验，阻塞操作必须 spawn_blocking。
    async fn execute(&self, name: &str, args: &Value, context: &ToolContext) -> Option<ToolResult>;
}

pub struct ToolRegistry {
    tools: Vec<Box<dyn AgentTool>>,
    name_index: HashMap<String, usize>,
    validators: HashMap<String, jsonschema::Validator>,
    dynamic_validators: Mutex<HashMap<String, (String, Arc<jsonschema::Validator>)>>,
    dynamic_provider: Option<Arc<dyn DynamicToolProvider>>,
}

impl ToolRegistry {
    pub(crate) fn new(tools: Vec<Box<dyn AgentTool>>) -> Self {
        let mut name_index = HashMap::new();
        let mut validators = HashMap::new();
        for (index, tool) in tools.iter().enumerate() {
            let duplicate = name_index.insert(tool.name().to_string(), index);
            assert!(
                duplicate.is_none(),
                "duplicate agent tool registered: {}",
                tool.name()
            );
            validators.insert(
                tool.name().to_string(),
                compile_builtin_schema(tool.name(), &tool.parameters()),
            );
        }
        Self {
            tools,
            name_index,
            validators,
            dynamic_validators: Mutex::new(HashMap::new()),
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
        let validator = compile_builtin_schema(tool.name(), &tool.parameters());
        let duplicate = self
            .name_index
            .insert(tool.name().to_string(), self.tools.len());
        assert!(
            duplicate.is_none(),
            "duplicate agent tool registered: {}",
            tool.name()
        );
        self.validators.insert(tool.name().to_string(), validator);
        self.tools.push(tool);
    }

    fn find_by_name(&self, name: &str) -> Option<&dyn AgentTool> {
        self.name_index
            .get(name)
            .and_then(|&idx| self.tools.get(idx))
            .map(Box::as_ref)
    }

    pub fn tool_names_and_descriptions(&self) -> Vec<(String, String)> {
        self.tools
            .iter()
            .map(|t| (t.name().to_string(), t.description().to_string()))
            .collect()
    }

    pub fn definitions_for_scope<'a, I>(
        &self,
        scope: &McpScope,
        allowed: Option<I>,
        include_dynamic: bool,
    ) -> Vec<ToolDefinition>
    where
        I: IntoIterator<Item = &'a str>,
    {
        self.specs_for_scope(scope, allowed, include_dynamic)
            .into_iter()
            .map(|spec| spec.to_definition())
            .collect()
    }

    pub fn specs_for_scope<'a, I>(
        &self,
        scope: &McpScope,
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
                        .specs_for_scope(scope)
                        .into_iter()
                        // 与内置工具重名的动态工具在执行期会被 find_by_name 优先命中
                        // 内置实现而静默遮蔽，定义层面同样去重，避免向 LLM 暴露
                        // 两份同名定义造成调用歧义。
                        .filter(|spec| !self.name_index.contains_key(&spec.name)),
                );
                // 动态（MCP）工具在本层不受 allowed 白名单约束：白名单只约束
                // 内置工具。聊天场景对 MCP 工具的分类门禁在消费端完成
                // （PlainChatAgent 的 retain_allowed_definitions，显式名单制）；
                // 服务器级启停在 MCP 注册表层治理（设置中心全局开关 /
                // 项目启停开关）。
            }
        }
        specs
    }

    pub fn spec_by_name(
        &self,
        scope: &McpScope,
        name: &str,
        include_dynamic: bool,
    ) -> Option<ToolSpec> {
        if let Some(tool) = self.find_by_name(name) {
            return Some(tool.spec());
        }
        if include_dynamic {
            if let Some(provider) = &self.dynamic_provider {
                return provider
                    .specs_for_scope(scope)
                    .into_iter()
                    .find(|spec| spec.name == name);
            }
        }
        None
    }

    pub fn is_parallel_readonly(
        &self,
        scope: &McpScope,
        name: &str,
        include_dynamic: bool,
    ) -> bool {
        self.spec_by_name(scope, name, include_dynamic)
            .is_some_and(|spec| spec.supports_parallel_readonly())
    }

    pub(super) async fn execute_input(
        &self,
        input: ToolInput,
        context: &ToolContext,
    ) -> ToolResult {
        let ToolInput {
            name,
            effective_arguments,
        } = input;

        // 必须使用 effective_arguments（补齐 schema 默认值后的参数）执行：
        // 与 runtime.rs 落库的 effective_arguments_json、摘要压缩路径保持同一套参数，
        // 避免「执行的参数」与「记录/压缩的参数」不一致。
        match self.find_by_name(&name) {
            Some(tool) => tool.execute(&effective_arguments, context).await,
            None => {
                if let Some(provider) = &self.dynamic_provider {
                    if let Some(result) =
                        provider.execute(&name, &effective_arguments, context).await
                    {
                        return result;
                    }
                }
                ToolResult::recoverable_error(format!("错误：未找到工具 '{}'", name))
            }
        }
    }

    /// 将模型原始参数转换为唯一、可执行的 ToolInput。
    ///
    /// 顺序固定为：查找工具规范 → 递归补 schema default → Draft 2020-12
    /// 严格校验。所有真实执行都必须经过此入口，工具实现只会收到
    /// effective_arguments，绝不能自行绕过校验执行原始参数。
    pub(super) fn prepare_input(
        &self,
        scope: &McpScope,
        tool_name: &str,
        args: &Value,
        include_dynamic: bool,
    ) -> Result<ToolInput, Box<ToolResult>> {
        let Some(spec) = self.spec_by_name(scope, tool_name, include_dynamic) else {
            return Err(Box::new(ToolResult::recoverable_error(format!(
                "错误：未找到工具 '{tool_name}'"
            ))));
        };

        let mut effective_arguments = args.clone();
        apply_schema_defaults(&spec.parameters, &mut effective_arguments);

        let dynamic_validator: Option<Arc<jsonschema::Validator>>;
        let validator = if let Some(validator) = self.validators.get(tool_name) {
            validator
        } else {
            let fingerprint = spec.fingerprint();
            let cached_validator = self
                .dynamic_validators
                .lock()
                .get(tool_name)
                .filter(|(cached_fingerprint, _)| cached_fingerprint == &fingerprint)
                .map(|(_, validator)| Arc::clone(validator));
            let resolved_validator = match cached_validator {
                Some(validator) => validator,
                None => {
                    let validator = match jsonschema::draft202012::new(&spec.parameters) {
                        Ok(validator) => Arc::new(validator),
                        Err(error) => {
                            let mut result = ToolResult::recoverable_error(format!(
                                "错误：工具 '{tool_name}' 的参数 Schema 无效：{error}"
                            ));
                            result.metadata = json!({
                                "code": "invalid_tool_schema",
                                "toolName": tool_name,
                            });
                            return Err(Box::new(result));
                        }
                    };
                    self.dynamic_validators
                        .lock()
                        .insert(tool_name.to_string(), (fingerprint, Arc::clone(&validator)));
                    validator
                }
            };
            // 在分支外保持 Arc 存活，validator 引用仅覆盖本次同步校验。
            dynamic_validator = Some(resolved_validator);
            dynamic_validator
                .as_deref()
                .expect("dynamic validator assigned")
        };

        // 先全量收集再截断：total 必须反映真实错误总数——此前在 take(16)
        // 之后统计，「…共 N 处错误」最多只能报 16。
        let all_errors: Vec<_> = validator.iter_errors(&effective_arguments).collect();
        let total = all_errors.len();
        let errors: Vec<_> = all_errors
            .into_iter()
            .take(16)
            .map(|error| {
                let instance_path = error.instance_path().to_string();
                json!({
                    "path": if instance_path.is_empty() { "/" } else { instance_path.as_str() },
                    "schemaPath": error.schema_path().to_string(),
                    "message": describe_validation_error(&error),
                })
            })
            .collect();
        if !errors.is_empty() {
            // 摘要最多列 8 处：错误一次报全，模型单轮重试即可修完，
            // 避免「改一处、报一处」的逐条挤牙膏式重试。
            let mut summary = errors
                .iter()
                .take(MAX_SUMMARIZED_ERRORS)
                .map(|error| {
                    format!(
                        "{}: {}",
                        error["path"].as_str().unwrap_or("/"),
                        error["message"].as_str().unwrap_or("参数无效")
                    )
                })
                .collect::<Vec<_>>()
                .join("；");
            if total > MAX_SUMMARIZED_ERRORS {
                summary.push_str(&format!("；…共 {total} 处错误"));
            }
            let mut result = ToolResult::recoverable_error(format!(
                "错误：工具 '{tool_name}' 参数不符合 JSON Schema：{summary}"
            ));
            result.metadata = json!({
                "code": "invalid_arguments",
                "toolName": tool_name,
                "errors": errors,
            });
            return Err(Box::new(result));
        }

        Ok(ToolInput {
            name: tool_name.to_string(),
            effective_arguments,
        })
    }

    /// 为控制面协议处理器提供与 CapabilityBroker 完全相同的参数准备路径。
    ///
    /// submit_graph / graph_plan_report 等工具由编排器拦截，不能进入普通
    /// AgentTool::execute；但它们仍必须在任何协议副作用前完成默认值注入与
    /// Draft 2020-12 Schema 校验。只暴露最终参数，不泄露可执行 ToolInput，
    /// 因而真实数据面工具仍只能由 Broker 执行。
    pub(crate) fn prepare_control_arguments(
        &self,
        scope: &McpScope,
        tool_name: &str,
        args: &Value,
    ) -> Result<Value, Box<ToolResult>> {
        self.prepare_input(scope, tool_name, args, false)
            .map(|input| input.effective_arguments)
    }

    pub fn effective_args(&self, tool_name: &str, args: &Value) -> Value {
        let Some(tool) = self.find_by_name(tool_name) else {
            return args.clone();
        };
        let schema = tool.parameters();
        let mut result = args.clone();
        apply_schema_defaults(&schema, &mut result);
        result
    }
}

fn compile_builtin_schema(name: &str, schema: &Value) -> jsonschema::Validator {
    jsonschema::draft202012::new(schema)
        .unwrap_or_else(|error| panic!("invalid JSON Schema for builtin tool '{name}': {error}"))
}

/// 参数校验错误摘要最多列出的条数（完整明细仍在 metadata.errors）。
const MAX_SUMMARIZED_ERRORS: usize = 8;

/// 校验错误的人类/模型可读描述。
///
/// oneOf/anyOf 的顶层消息（「is not valid under any of the schemas listed in
/// the 'oneOf' keyword」）对修复毫无指引——判别式联合（如指令的 `_type`）
/// 通常只有一个分支接近匹配，该分支的子错误（如「Additional properties are
/// not allowed ('labelPosition' was unexpected)」）才是真正可操作的失败原因。
/// 这里展开「错误最少的分支」的子错误作为消息，让模型一轮即可定位字段。
fn describe_validation_error(error: &jsonschema::ValidationError<'_>) -> String {
    let context = match error.kind() {
        jsonschema::error::ValidationErrorKind::OneOfNotValid { context }
        | jsonschema::error::ValidationErrorKind::AnyOf { context } => context,
        _ => return error.to_string(),
    };
    let Some(closest) = context
        .iter()
        .filter(|branch| !branch.is_empty())
        .min_by_key(|branch| (discriminator_penalty(branch), branch.len()))
    else {
        return error.to_string();
    };
    let detail = closest
        .iter()
        .take(3)
        .map(|sub_error| sub_error.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    if closest.len() > 3 {
        format!("{detail}; …共 {} 处", closest.len())
    } else {
        detail
    }
}

/// 分支降权：子错误里带 `_type` 常量失配（判别式对不上）的分支几乎不可能
/// 是模型想表达的分支——正确分支的该计数恒为 0，`_type` 拼写错误时所有
/// 分支都会带一条。平局时先比降权再比错误数，避免固定命中声明顺序靠前的
/// 分支而给出误导性修复指引。
fn discriminator_penalty(branch: &[jsonschema::ValidationError<'_>]) -> usize {
    branch
        .iter()
        // 分支子错误携带从校验根起的完整路径（如 "/item/_type"），
        // 故按后缀匹配判别式字段，而非相对路径。
        .filter(|error| error.instance_path().to_string().ends_with("/_type"))
        .count()
}

/// JSON Schema 的 default 是注解，不会由 validator 自动写入实例。
/// 这里只执行确定性的 properties/items 递归：不会猜测 anyOf/oneOf 分支，
/// 也不会凭空创建没有自身 default 的父对象。
fn apply_schema_defaults(schema: &Value, instance: &mut Value) {
    match instance {
        Value::Object(object) => {
            let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
                return;
            };
            for (name, property_schema) in properties {
                if !object.contains_key(name) {
                    if let Some(default) = property_schema.get("default") {
                        object.insert(name.clone(), default.clone());
                    }
                }
                if let Some(value) = object.get_mut(name) {
                    apply_schema_defaults(property_schema, value);
                }
            }
        }
        Value::Array(items) => {
            if let Some(item_schema) = schema.get("items") {
                for item in items {
                    apply_schema_defaults(item_schema, item);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::{AgentTool, DynamicToolProvider, ToolRegistry};
    use crate::agent::llm::ToolDefinition;
    use crate::agent::tools::spec::ToolSpec;
    use crate::agent::tools::{ToolContext, ToolResult};
    use crate::mcp::McpScope;

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

        async fn execute(&self, _args: &Value, _context: &ToolContext) -> ToolResult {
            ToolResult::success_text("ok")
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

        async fn execute(&self, args: &Value, _context: &ToolContext) -> ToolResult {
            ToolResult::success_text(args.to_string())
        }
    }

    struct TestDynamicProvider {
        specs: Vec<ToolSpec>,
    }

    #[async_trait]
    impl DynamicToolProvider for TestDynamicProvider {
        fn definitions_for_scope(&self, _scope: &McpScope) -> Vec<ToolDefinition> {
            self.specs.iter().map(ToolSpec::to_definition).collect()
        }

        fn specs_for_scope(&self, _scope: &McpScope) -> Vec<ToolSpec> {
            self.specs.clone()
        }

        async fn execute(
            &self,
            name: &str,
            args: &Value,
            _context: &ToolContext,
        ) -> Option<ToolResult> {
            self.specs
                .iter()
                .find(|spec| spec.name == name)
                .map(|_| ToolResult::success_text(format!("dynamic:{args}")))
        }
    }

    fn test_context() -> ToolContext {
        ToolContext {
            workspace_id: "ws-test".to_string(),
            workspace: PathBuf::from("/tmp"),
            mcp_scope: McpScope::Global,
            session_title: "test".to_string(),
            user_task: None,
            executor_task: None,
            review_conversation: None,
            ssh_review: None,
            exec_timeout_secs: 60,
            restrict_to_workspace: true,
            extra_allowed_dirs: Vec::new(),
            app_handle: None,
            llm_provider: None,
            vision_model: String::new(),
            vision_provider: None,
            image_model_url: String::new(),
            image_model_api_key: String::new(),
            image_model: String::new(),
            image_edit_model: String::new(),
            sub_agent_tool_registry: None,
            current_sub_agent_id: None,
            current_sub_agent_name: None,
            current_tool_call_id: None,
            current_tool_spec_hash: None,
            cancel_rx: None,
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

        assert!(registry.is_parallel_readonly(&McpScope::Global, "read_file", false));
        assert!(!registry.is_parallel_readonly(&McpScope::Global, "missing", false));
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
    fn schema_defaults_are_applied_recursively_to_existing_objects_and_arrays() {
        let schema = json!({
            "type": "object",
            "properties": {
                "nested": {
                    "type": "object",
                    "properties": {
                        "mode": { "type": "string", "default": "safe" }
                    }
                },
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "enabled": { "type": "boolean", "default": true }
                        }
                    }
                }
            }
        });
        let mut instance = json!({ "nested": {}, "items": [{}, { "enabled": false }] });

        super::apply_schema_defaults(&schema, &mut instance);

        assert_eq!(instance["nested"]["mode"], "safe");
        assert_eq!(instance["items"][0]["enabled"], true);
        assert_eq!(instance["items"][1]["enabled"], false);
        assert!(instance.get("missing_parent").is_none());
    }

    #[test]
    fn prepare_input_rejects_invalid_arguments_with_structured_metadata() {
        let registry = ToolRegistry::new(vec![Box::new(TestTool { name: "read_file" })]);

        let error = registry
            .prepare_input(
                &McpScope::Global,
                "read_file",
                &json!({ "flag": "not-a-boolean" }),
                false,
            )
            .unwrap_err();

        assert_eq!(
            error.status,
            crate::agent::tools::result::ToolStatus::RecoverableError
        );
        assert_eq!(error.metadata["code"], "invalid_arguments");
        assert_eq!(error.metadata["toolName"], "read_file");
        assert!(error.metadata["errors"]
            .as_array()
            .is_some_and(|v| !v.is_empty()));
    }

    /// oneOf 判别式联合的参数错误必须展开为「最接近分支」的字段级子错误，
    /// 而不是笼统的 "is not valid under any of the schemas listed in the 'oneOf' keyword"——
    /// 后者让模型无从修起，曾导致画布程序反复重试。
    #[test]
    fn prepare_input_expands_oneof_errors_to_closest_branch_details() {
        struct OneOfTool;
        #[async_trait]
        impl AgentTool for OneOfTool {
            fn name(&self) -> &'static str {
                "one_of_tool"
            }
            fn description(&self) -> &'static str {
                "oneof test tool"
            }
            fn parameters(&self) -> Value {
                json!({
                    "type": "object",
                    "required": ["item"],
                    "properties": {
                        "item": {
                            "oneOf": [
                                {
                                    "type": "object",
                                    "additionalProperties": false,
                                    "required": ["_type", "name"],
                                    "properties": {
                                        "_type": { "const": "alpha" },
                                        "name": { "type": "string" },
                                    },
                                },
                                {
                                    "type": "object",
                                    "additionalProperties": false,
                                    "required": ["_type", "count"],
                                    "properties": {
                                        "_type": { "const": "beta" },
                                        "count": { "type": "integer" },
                                    },
                                },
                            ],
                        },
                    },
                })
            }
            async fn execute(&self, _args: &Value, _context: &ToolContext) -> ToolResult {
                ToolResult::success_text("ok")
            }
        }

        let registry = ToolRegistry::new(vec![Box::new(OneOfTool)]);
        // _type=beta 但多带了 alpha 分支的 name 字段：最接近分支是 beta，
        // 子错误应精确指出 name 不合法。
        let error = registry
            .prepare_input(
                &McpScope::Global,
                "one_of_tool",
                &json!({ "item": { "_type": "beta", "count": 2, "name": "x" } }),
                false,
            )
            .unwrap_err();

        let message = error.display.clone();
        assert!(
            !message.contains("not valid under any of the schemas"),
            "不应再暴露笼统的 oneOf 消息：{message}"
        );
        assert!(message.contains("name"), "应展开到具体字段：{message}");
        let detail = error.metadata["errors"][0]["message"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(detail.contains("name"), "{detail}");
    }

    /// 分支错误数打平时不得固定命中声明顺序靠前的分支：判别式 `_type`
    /// 匹配的分支优先——否则声明在前的分支会在平局时抢走错误消息，
    /// 给出误导性修复指引（拿 alpha 分支的 count 说事，而模型想要 beta）。
    #[test]
    fn prepare_input_prefers_discriminator_matching_branch_on_tie() {
        struct TieTool;
        #[async_trait]
        impl AgentTool for TieTool {
            fn name(&self) -> &'static str {
                "tie_tool"
            }
            fn description(&self) -> &'static str {
                "tie-break test tool"
            }
            fn parameters(&self) -> Value {
                json!({
                    "type": "object",
                    "required": ["item"],
                    "properties": {
                        "item": {
                            "oneOf": [
                                {
                                    "type": "object",
                                    "additionalProperties": false,
                                    "required": ["_type", "name"],
                                    "properties": {
                                        "_type": { "const": "alpha" },
                                        "name": { "type": "string" },
                                    },
                                },
                                {
                                    "type": "object",
                                    "additionalProperties": false,
                                    "required": ["_type", "count", "extra"],
                                    "properties": {
                                        "_type": { "const": "beta" },
                                        "count": { "type": "integer" },
                                        "extra": { "type": "string" },
                                    },
                                },
                            ],
                        },
                    },
                })
            }
            async fn execute(&self, _args: &Value, _context: &ToolContext) -> ToolResult {
                ToolResult::success_text("ok")
            }
        }

        let registry = ToolRegistry::new(vec![Box::new(TieTool)]);
        // _type=beta：beta 分支（多余 name + 缺 extra）与 alpha 分支（多余
        // count + _type 失配）各 2 处错误打平，但只有 beta 与判别式一致。
        let error = registry
            .prepare_input(
                &McpScope::Global,
                "tie_tool",
                &json!({ "item": { "_type": "beta", "count": 2, "name": "x" } }),
                false,
            )
            .unwrap_err();

        let message = error.display.clone();
        assert!(
            message.contains("name") || message.contains("extra"),
            "应指向 beta 分支的字段问题：{message}"
        );
        assert!(
            !message.contains("count"),
            "不应误报 alpha 分支才有的 count 问题：{message}"
        );
    }

    /// 端到端：模型拿 update_shape 改箭头 labelPosition（历史真实故障）时，
    /// 错误消息必须直接点出 labelPosition 字段，而不是笼统的 oneOf 文本。
    #[test]
    fn architecture_run_reports_labelposition_field_error() {
        let registry = ToolRegistry::architecture_tools();
        let error = registry
            .prepare_input(
                &McpScope::Global,
                "architecture_run",
                &json!({
                    "program": {
                        "version": 1,
                        "instructions": [
                            {
                                "_type": "update_shape",
                                "labelPosition": 0.3,
                                "target": "shape:jIpSCG3QVzhAw6bjPa2iw"
                            },
                        ],
                    },
                }),
                false,
            )
            .unwrap_err();
        assert!(
            error.display.contains("labelPosition"),
            "错误应点出具体字段：{}",
            error.display
        );
        assert!(
            !error.display.contains("not valid under any of the schemas"),
            "不应保留笼统 oneOf 文本：{}",
            error.display
        );
    }

    #[test]
    fn orchestrator_runtime_tools_reject_unbounded_fanout_before_execution() {
        let registry = ToolRegistry::orchestrator_tools();
        let cases = [
            (
                "read_file",
                json!({ "paths": (0..9).map(|index| format!("{index}.rs")).collect::<Vec<_>>() }),
            ),
            ("list_dir", json!({ "paths": ["."], "max_entries": 201 })),
            (
                "glob",
                json!({ "patterns": ["a", "b", "c", "d", "e"], "paths": ["."] }),
            ),
            (
                "grep",
                json!({ "pattern": "needle", "paths": ["."], "max_files": 201 }),
            ),
        ];

        for (tool_name, arguments) in cases {
            let result = registry
                .prepare_input(&McpScope::Global, tool_name, &arguments, false)
                .expect_err("oversized runtime fanout must fail schema validation");
            assert_eq!(result.metadata["code"], "invalid_arguments", "{tool_name}");
        }
    }

    #[test]
    fn prepare_input_applies_defaults_and_validates_dynamic_tools() {
        let registry =
            ToolRegistry::new(Vec::new()).with_dynamic_provider(Arc::new(TestDynamicProvider {
                specs: vec![ToolSpec::mcp(
                    "mcp__demo__tool".to_string(),
                    "动态工具".to_string(),
                    json!({
                        "type": "object",
                        "properties": {
                            "flag": { "type": "boolean", "default": true }
                        },
                        "required": ["flag"]
                    }),
                )],
            }));

        let input = registry
            .prepare_input(&McpScope::Global, "mcp__demo__tool", &json!({}), true)
            .unwrap();

        assert_eq!(input.effective_arguments["flag"], true);
    }

    /// allowed 白名单只约束内置（静态）工具：动态（MCP）工具名称随服务器
    /// 配置动态生成，静态白名单无法表达，其启停治理在 MCP 注册表层。
    #[test]
    fn allowed_whitelist_governs_only_builtin_specs() {
        let registry = ToolRegistry::new(vec![Box::new(TestTool { name: "read_file" })])
            .with_dynamic_provider(Arc::new(TestDynamicProvider {
                specs: vec![ToolSpec::mcp(
                    "mcp__demo__tool".to_string(),
                    "动态工具".to_string(),
                    json!({ "type": "object", "properties": {} }),
                )],
            }));

        // 白名单未包含动态工具时，动态工具仍然放行（白名单管不到）。
        let filtered =
            registry.specs_for_scope(&McpScope::Global, Some(["read_file"].into_iter()), true);
        assert_eq!(
            filtered.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["read_file", "mcp__demo__tool"]
        );

        // 白名单收窄内置工具：未列出的内置工具被过滤，动态工具不受影响。
        let builtin_excluded = registry.specs_for_scope(
            &McpScope::Global,
            Some(["mcp__demo__tool"].into_iter()),
            true,
        );
        assert_eq!(
            builtin_excluded
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            vec!["mcp__demo__tool"]
        );

        // 无白名单时全部放行。
        let all = registry.specs_for_scope(
            &McpScope::Global,
            Option::<std::iter::Empty<&str>>::None,
            true,
        );
        assert_eq!(all.len(), 2);
    }

    /// 动态（MCP）工具按作用域枚举：项目专属服务器只在项目作用域可见，
    /// 全局作用域（聊天）不可见——替代旧实现「按工作区路径推断」。
    struct ScopeSensitiveProvider {
        project_only: ToolSpec,
    }

    #[async_trait]
    impl DynamicToolProvider for ScopeSensitiveProvider {
        fn definitions_for_scope(&self, scope: &McpScope) -> Vec<ToolDefinition> {
            self.specs_for_scope(scope)
                .iter()
                .map(ToolSpec::to_definition)
                .collect()
        }

        fn specs_for_scope(&self, scope: &McpScope) -> Vec<ToolSpec> {
            match scope {
                McpScope::Project(_) => vec![self.project_only.clone()],
                McpScope::Global => Vec::new(),
            }
        }

        async fn execute(
            &self,
            _name: &str,
            _args: &Value,
            _context: &ToolContext,
        ) -> Option<ToolResult> {
            None
        }
    }

    #[test]
    fn dynamic_specs_follow_scope_not_path() {
        let registry =
            ToolRegistry::new(Vec::new()).with_dynamic_provider(Arc::new(ScopeSensitiveProvider {
                project_only: ToolSpec::mcp(
                    "mcp__proj__tool".to_string(),
                    "项目专属工具".to_string(),
                    json!({ "type": "object", "properties": {} }),
                ),
            }));

        let project = McpScope::Project(std::env::temp_dir());
        assert!(registry
            .spec_by_name(&project, "mcp__proj__tool", true)
            .is_some());
        assert!(registry
            .spec_by_name(&McpScope::Global, "mcp__proj__tool", true)
            .is_none());
        assert_eq!(
            registry
                .specs_for_scope(&project, Option::<std::iter::Empty<&str>>::None, true)
                .len(),
            1
        );
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

        let specs = registry.specs_for_scope(
            &McpScope::Global,
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

        let input = registry
            .prepare_input(&context.mcp_scope, "echo", &json!({}), false)
            .unwrap();
        let result = registry.execute_input(input, &context).await;

        // 工具实际收到的参数必须包含 schema 默认值（effective_arguments）。
        assert_eq!(
            result.status,
            crate::agent::tools::result::ToolStatus::Success
        );
        assert_eq!(result.display, "{\"flag\":true}");
    }

    #[tokio::test]
    async fn prepare_input_reports_unknown_tool_with_error_prefix() {
        let registry = ToolRegistry::new(vec![Box::new(TestTool { name: "read_file" })]);
        let context = test_context();

        let result = registry
            .prepare_input(&context.mcp_scope, "missing_tool", &json!({}), false)
            .unwrap_err();

        assert_eq!(
            result.status,
            crate::agent::tools::result::ToolStatus::RecoverableError
        );
        assert!(result.display.starts_with("错误："));
    }
}

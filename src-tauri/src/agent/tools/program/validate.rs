use std::collections::BTreeSet;

use serde_json::Value;

use super::ast::{ProgramNode, ToolProgram, TOOL_PROGRAM_VERSION};
use super::error::{ProgramError, ProgramErrorKind};
use super::value::visit_references_at;

pub const CONTROL_PLANE_TOOLS: &[&str] = &[
    "run_tool_program",
    "message",
    "submit_graph",
    "graph_plan_report",
    "notify_user_progress",
    "call_sub_agent",
];

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct CapabilityPolicy {
    pub supports_parallel_readonly: bool,
}

impl CapabilityPolicy {
    #[cfg(test)]
    pub const fn sequential() -> Self {
        Self {
            supports_parallel_readonly: false,
        }
    }

    #[cfg(test)]
    pub const fn parallel_readonly() -> Self {
        Self {
            supports_parallel_readonly: true,
        }
    }
}

/// 验证器只依赖能力快照，不依赖 ToolRegistry/ToolSpec。
/// 真正执行时 CapabilityBroker 必须再次授权，避免验证与执行之间的策略漂移。
pub trait CapabilityCatalog {
    fn capability(&self, tool_name: &str) -> Option<CapabilityPolicy>;
}

impl<F> CapabilityCatalog for F
where
    F: Fn(&str) -> Option<CapabilityPolicy>,
{
    fn capability(&self, tool_name: &str) -> Option<CapabilityPolicy> {
        self(tool_name)
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProgramLimits {
    pub max_input_bytes: usize,
    pub max_nodes: usize,
    pub max_calls: usize,
    pub max_depth: usize,
    pub max_parallel_branches: usize,
    pub max_concurrency: usize,
    pub max_resolved_arguments_bytes: usize,
    pub max_step_envelope_bytes: usize,
    pub max_environment_bytes: usize,
    pub max_return_bytes: usize,
    pub max_wall_time_secs: u64,
    /// 达到 wall-time 后等待已启动 Broker 调用协作收敛的硬上限。
    pub max_drain_time_ms: u64,
}

impl Default for ProgramLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 64 * 1024,
            max_nodes: 64,
            max_calls: 32,
            max_depth: 6,
            max_parallel_branches: 8,
            max_concurrency: 4,
            max_resolved_arguments_bytes: 64 * 1024,
            max_step_envelope_bytes: 256 * 1024,
            max_environment_bytes: 1024 * 1024,
            max_return_bytes: 64 * 1024,
            max_wall_time_secs: 120,
            max_drain_time_ms: 5_000,
        }
    }
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
struct ProgramStats {
    pub node_count: usize,
    pub call_count: usize,
    pub max_depth: usize,
    pub max_parallel_width: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedProgram {
    program: ToolProgram,
}

impl ValidatedProgram {
    pub fn program(&self) -> &ToolProgram {
        &self.program
    }
}

#[cfg(test)]
pub fn parse_and_validate_program<C: CapabilityCatalog + ?Sized>(
    input: &[u8],
    catalog: &C,
    limits: &ProgramLimits,
) -> Result<ValidatedProgram, ProgramError> {
    validate_limits(limits)?;
    if input.len() > limits.max_input_bytes {
        return Err(limit_error(format!(
            "程序输入为 {} 字节，超过上限 {} 字节",
            input.len(),
            limits.max_input_bytes
        )));
    }
    let program: ToolProgram = serde_json::from_slice(input).map_err(|error| {
        ProgramError::new(
            ProgramErrorKind::Parse,
            format!("ToolProgram JSON 解析失败：{error}"),
        )
    })?;
    validate_program_inner(program, catalog, limits)
}

pub fn validate_program_value<C: CapabilityCatalog + ?Sized>(
    input: &Value,
    catalog: &C,
    limits: &ProgramLimits,
) -> Result<ValidatedProgram, ProgramError> {
    validate_limits(limits)?;
    let bytes = serde_json::to_vec(input).map_err(|error| {
        ProgramError::new(
            ProgramErrorKind::Parse,
            format!("ToolProgram JSON 序列化失败：{error}"),
        )
    })?;
    if bytes.len() > limits.max_input_bytes {
        return Err(limit_error(format!(
            "程序输入为 {} 字节，超过上限 {} 字节",
            bytes.len(),
            limits.max_input_bytes
        )));
    }
    let program = serde_json::from_value(input.clone()).map_err(|error| {
        ProgramError::new(
            ProgramErrorKind::Parse,
            format!("ToolProgram JSON 解析失败：{error}"),
        )
    })?;
    validate_program_inner(program, catalog, limits)
}

fn validate_program_inner<C: CapabilityCatalog + ?Sized>(
    program: ToolProgram,
    catalog: &C,
    limits: &ProgramLimits,
) -> Result<ValidatedProgram, ProgramError> {
    if program.version != TOOL_PROGRAM_VERSION {
        return Err(ProgramError::new(
            ProgramErrorKind::Validation,
            format!(
                "不支持 ToolProgram 版本 {}，当前仅支持版本 {}",
                program.version, TOOL_PROGRAM_VERSION
            ),
        ));
    }

    validate_root_shape(&program.root)?;

    let mut state = ValidationState::new(catalog, limits);
    let mut available = BTreeSet::new();
    state.validate_node(&program.root, &mut available, 1, "/root", false, false)?;

    Ok(ValidatedProgram { program })
}

fn validate_root_shape(root: &ProgramNode) -> Result<(), ProgramError> {
    let ProgramNode::Sequence { steps } = root else {
        return Err(ProgramError::new(
            ProgramErrorKind::Validation,
            "ToolProgram 根节点必须是 sequence",
        )
        .at_path("/root"));
    };
    if steps.is_empty() {
        return Err(
            ProgramError::new(ProgramErrorKind::Validation, "根 sequence 不能为空")
                .at_path("/root/steps"),
        );
    }
    if !matches!(steps.last(), Some(ProgramNode::Return { .. })) {
        return Err(ProgramError::new(
            ProgramErrorKind::Validation,
            "根 sequence 的最后一个节点必须是 return",
        )
        .at_path("/root/steps"));
    }

    Ok(())
}

struct ValidationState<'a, C: CapabilityCatalog + ?Sized> {
    catalog: &'a C,
    limits: &'a ProgramLimits,
    stats: ProgramStats,
    seen_step_ids: BTreeSet<String>,
}

impl<'a, C: CapabilityCatalog + ?Sized> ValidationState<'a, C> {
    fn new(catalog: &'a C, limits: &'a ProgramLimits) -> Self {
        Self {
            catalog,
            limits,
            stats: ProgramStats::default(),
            seen_step_ids: BTreeSet::new(),
        }
    }

    fn validate_node(
        &mut self,
        node: &ProgramNode,
        available: &mut BTreeSet<String>,
        depth: usize,
        path: &str,
        inside_parallel: bool,
        return_allowed: bool,
    ) -> Result<(), ProgramError> {
        self.stats.node_count += 1;
        self.stats.max_depth = self.stats.max_depth.max(depth);
        if self.stats.node_count > self.limits.max_nodes {
            return Err(
                limit_error(format!("程序节点数超过上限 {}", self.limits.max_nodes)).at_path(path),
            );
        }
        if depth > self.limits.max_depth {
            return Err(limit_error(format!(
                "程序嵌套深度 {depth} 超过上限 {}",
                self.limits.max_depth
            ))
            .at_path(path));
        }

        match node {
            ProgramNode::Call {
                id,
                tool,
                arguments,
            } => self.validate_call(id, tool, arguments, available, path, inside_parallel),
            ProgramNode::Sequence { steps } => {
                if steps.is_empty() {
                    return Err(ProgramError::new(
                        ProgramErrorKind::Validation,
                        "sequence 不能为空",
                    )
                    .at_path(path));
                }
                for (index, step) in steps.iter().enumerate() {
                    self.validate_node(
                        step,
                        available,
                        depth + 1,
                        &format!("{path}/steps/{index}"),
                        inside_parallel,
                        path == "/root" && index + 1 == steps.len(),
                    )?;
                }
                Ok(())
            }
            ProgramNode::Parallel { branches } => {
                if branches.len() < 2 {
                    return Err(ProgramError::new(
                        ProgramErrorKind::Validation,
                        "parallel 至少需要两个 branch",
                    )
                    .at_path(path));
                }
                if branches.len() > self.limits.max_parallel_branches {
                    return Err(limit_error(format!(
                        "parallel branch 数 {} 超过上限 {}",
                        branches.len(),
                        self.limits.max_parallel_branches
                    ))
                    .at_path(path));
                }
                self.stats.max_parallel_width = self.stats.max_parallel_width.max(branches.len());

                // 每个 branch 只能看到进入 parallel 前的同一份快照；兄弟 branch
                // 在验证顺序上即使已经出现，也不会泄漏进后续 branch 的可见集合。
                let entry_snapshot = available.clone();
                let mut defined_by_branches = BTreeSet::new();
                for (index, branch) in branches.iter().enumerate() {
                    let mut branch_available = entry_snapshot.clone();
                    self.validate_node(
                        branch,
                        &mut branch_available,
                        depth + 1,
                        &format!("{path}/branches/{index}"),
                        true,
                        false,
                    )?;
                    defined_by_branches.extend(
                        branch_available
                            .difference(&entry_snapshot)
                            .cloned()
                            .collect::<Vec<_>>(),
                    );
                }
                available.extend(defined_by_branches);
                Ok(())
            }
            ProgramNode::Return { value } => {
                if !return_allowed {
                    return Err(ProgramError::new(
                        ProgramErrorKind::Validation,
                        "return 只能作为根 sequence 的最后一个节点",
                    )
                    .at_path(path));
                }
                let size = json_size(value)?;
                if size > self.limits.max_return_bytes {
                    return Err(limit_error(format!(
                        "return 模板为 {size} 字节，超过上限 {} 字节",
                        self.limits.max_return_bytes
                    ))
                    .at_path(path));
                }
                validate_template_references(value, available, &format!("{path}/value"))
            }
        }
    }

    fn validate_call(
        &mut self,
        id: &str,
        tool: &str,
        arguments: &Value,
        available: &mut BTreeSet<String>,
        path: &str,
        inside_parallel: bool,
    ) -> Result<(), ProgramError> {
        self.stats.call_count += 1;
        if self.stats.call_count > self.limits.max_calls {
            return Err(
                limit_error(format!("工具调用数超过上限 {}", self.limits.max_calls)).at_path(path),
            );
        }
        if !valid_step_id(id) {
            return Err(ProgramError::new(
                ProgramErrorKind::Validation,
                format!("步骤 ID '{id}' 非法；必须匹配 [A-Za-z][A-Za-z0-9_-]{{0,63}}"),
            )
            .at_path(path)
            .for_step(id, tool));
        }
        if !self.seen_step_ids.insert(id.to_string()) {
            return Err(ProgramError::new(
                ProgramErrorKind::Validation,
                format!("步骤 ID '{id}' 重复"),
            )
            .at_path(path)
            .for_step(id, tool));
        }
        if tool.trim().is_empty() {
            return Err(
                ProgramError::new(ProgramErrorKind::Validation, "工具名不能为空")
                    .at_path(path)
                    .for_step(id, tool),
            );
        }
        if CONTROL_PLANE_TOOLS.contains(&tool) {
            return Err(ProgramError::new(
                ProgramErrorKind::PolicyDenied,
                format!("控制面工具 '{tool}' 不允许在 ToolProgram 内调用"),
            )
            .at_path(path)
            .for_step(id, tool));
        }
        if !arguments.is_object() {
            return Err(ProgramError::new(
                ProgramErrorKind::Validation,
                "call.arguments 必须是 JSON object",
            )
            .at_path(format!("{path}/arguments"))
            .for_step(id, tool));
        }

        let arguments_size = json_size(arguments)?;
        if arguments_size > self.limits.max_resolved_arguments_bytes {
            return Err(limit_error(format!(
                "步骤 '{id}' 的参数模板为 {arguments_size} 字节，超过参数上限 {} 字节",
                self.limits.max_resolved_arguments_bytes
            ))
            .at_path(format!("{path}/arguments"))
            .for_step(id, tool));
        }
        validate_template_references(arguments, available, &format!("{path}/arguments"))?;

        let capability = self.catalog.capability(tool).ok_or_else(|| {
            ProgramError::new(
                ProgramErrorKind::PolicyDenied,
                format!("工具 '{tool}' 不在当前 capability grant 中"),
            )
            .at_path(path)
            .for_step(id, tool)
        })?;
        if inside_parallel && !capability.supports_parallel_readonly {
            return Err(ProgramError::new(
                ProgramErrorKind::PolicyDenied,
                format!("工具 '{tool}' 不支持只读并行执行"),
            )
            .at_path(path)
            .for_step(id, tool));
        }

        available.insert(id.to_string());
        Ok(())
    }
}

fn validate_template_references(
    template: &Value,
    available: &BTreeSet<String>,
    base_path: &str,
) -> Result<(), ProgramError> {
    visit_references_at(template, base_path, &mut |reference, reference_path| {
        if available.contains(&reference.step) {
            return Ok(());
        }
        Err(ProgramError::new(
            ProgramErrorKind::InvalidReference,
            format!("步骤 '{}' 不存在，或在当前位置尚未确定完成", reference.step),
        )
        .at_path(reference_path))
    })
}

fn valid_step_id(id: &str) -> bool {
    if id.is_empty() || id.len() > 64 || !id.is_ascii() {
        return false;
    }
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
}

fn validate_limits(limits: &ProgramLimits) -> Result<(), ProgramError> {
    let invalid = limits.max_input_bytes == 0
        || limits.max_nodes == 0
        || limits.max_calls == 0
        || limits.max_depth == 0
        || limits.max_parallel_branches < 2
        || limits.max_concurrency == 0
        || limits.max_resolved_arguments_bytes == 0
        || limits.max_step_envelope_bytes == 0
        || limits.max_environment_bytes == 0
        || limits.max_return_bytes == 0
        || limits.max_wall_time_secs == 0
        || limits.max_drain_time_ms == 0
        || limits.max_drain_time_ms > 5_000;
    if invalid {
        return Err(ProgramError::new(
            ProgramErrorKind::Internal,
            "ProgramLimits 必须全部为正数，max_parallel_branches 至少为 2，max_drain_time_ms 不得超过 5000",
        ));
    }
    Ok(())
}

fn json_size(value: &Value) -> Result<usize, ProgramError> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|error| {
            ProgramError::new(
                ProgramErrorKind::Internal,
                format!("计算 JSON 预算失败：{error}"),
            )
        })
}

fn limit_error(message: impl Into<String>) -> ProgramError {
    ProgramError::new(ProgramErrorKind::LimitExceeded, message)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::super::error::ProgramErrorKind;
    use super::{
        parse_and_validate_program, validate_program_value, CapabilityCatalog, CapabilityPolicy,
        ProgramLimits,
    };

    struct Catalog(BTreeMap<&'static str, CapabilityPolicy>);

    impl Catalog {
        fn standard() -> Self {
            Self(BTreeMap::from([
                ("read_file", CapabilityPolicy::parallel_readonly()),
                ("grep", CapabilityPolicy::parallel_readonly()),
                ("write_file", CapabilityPolicy::sequential()),
            ]))
        }
    }

    impl CapabilityCatalog for Catalog {
        fn capability(&self, tool_name: &str) -> Option<CapabilityPolicy> {
            self.0.get(tool_name).copied()
        }
    }

    fn valid_program() -> serde_json::Value {
        json!({
            "version": 1,
            "root": {
                "op": "sequence",
                "steps": [
                    {
                        "op": "call",
                        "id": "search",
                        "tool": "grep",
                        "arguments": { "pattern": "ToolRuntime" }
                    },
                    {
                        "op": "parallel",
                        "branches": [
                            {
                                "op": "call",
                                "id": "left",
                                "tool": "read_file",
                                "arguments": {
                                    "path": { "$ref": { "step": "search", "pointer": "/output" } }
                                }
                            },
                            {
                                "op": "sequence",
                                "steps": [
                                    {
                                        "op": "call",
                                        "id": "right",
                                        "tool": "read_file",
                                        "arguments": { "path": "src/lib.rs" }
                                    }
                                ]
                            }
                        ]
                    },
                    {
                        "op": "return",
                        "value": {
                            "left": { "$ref": { "step": "left", "pointer": "/output" } },
                            "right": { "$ref": { "step": "right", "pointer": "/output" } }
                        }
                    }
                ]
            }
        })
    }

    #[test]
    fn accepts_valid_program() {
        validate_program_value(
            &valid_program(),
            &Catalog::standard(),
            &ProgramLimits::default(),
        )
        .expect("valid program");
    }

    #[test]
    fn rejects_wrong_version_root_and_return_placement() {
        let mut wrong_version = valid_program();
        wrong_version["version"] = json!(2);
        assert_eq!(
            validate_program_value(
                &wrong_version,
                &Catalog::standard(),
                &ProgramLimits::default()
            )
            .unwrap_err()
            .kind,
            ProgramErrorKind::Validation
        );

        let non_sequence_root = json!({
            "version": 1,
            "root": { "op": "return", "value": null }
        });
        assert_eq!(
            validate_program_value(
                &non_sequence_root,
                &Catalog::standard(),
                &ProgramLimits::default()
            )
            .unwrap_err()
            .kind,
            ProgramErrorKind::Validation
        );

        let nested_return = json!({
            "version": 1,
            "root": {
                "op": "sequence",
                "steps": [
                    {
                        "op": "sequence",
                        "steps": [{ "op": "return", "value": null }]
                    },
                    { "op": "return", "value": null }
                ]
            }
        });
        assert_eq!(
            validate_program_value(
                &nested_return,
                &Catalog::standard(),
                &ProgramLimits::default()
            )
            .unwrap_err()
            .kind,
            ProgramErrorKind::Validation
        );
    }

    #[test]
    fn enforces_definite_before_use_in_sequence() {
        let forward_reference = json!({
            "version": 1,
            "root": {
                "op": "sequence",
                "steps": [
                    {
                        "op": "call",
                        "id": "first",
                        "tool": "read_file",
                        "arguments": {
                            "path": { "$ref": { "step": "later", "pointer": "/output" } }
                        }
                    },
                    {
                        "op": "call",
                        "id": "later",
                        "tool": "read_file",
                        "arguments": { "path": "a.rs" }
                    },
                    { "op": "return", "value": null }
                ]
            }
        });

        let error = validate_program_value(
            &forward_reference,
            &Catalog::standard(),
            &ProgramLimits::default(),
        )
        .unwrap_err();
        assert_eq!(error.kind, ProgramErrorKind::InvalidReference);
    }

    #[test]
    fn parallel_branches_cannot_reference_siblings() {
        let sibling_reference = json!({
            "version": 1,
            "root": {
                "op": "sequence",
                "steps": [
                    {
                        "op": "parallel",
                        "branches": [
                            {
                                "op": "call",
                                "id": "left",
                                "tool": "read_file",
                                "arguments": { "path": "a.rs" }
                            },
                            {
                                "op": "call",
                                "id": "right",
                                "tool": "read_file",
                                "arguments": {
                                    "path": { "$ref": { "step": "left", "pointer": "/output" } }
                                }
                            }
                        ]
                    },
                    { "op": "return", "value": null }
                ]
            }
        });

        let error = validate_program_value(
            &sibling_reference,
            &Catalog::standard(),
            &ProgramLimits::default(),
        )
        .unwrap_err();
        assert_eq!(error.kind, ProgramErrorKind::InvalidReference);
    }

    #[test]
    fn parallel_outputs_are_available_after_join() {
        let validated = validate_program_value(
            &valid_program(),
            &Catalog::standard(),
            &ProgramLimits::default(),
        );
        assert!(validated.is_ok());
    }

    #[test]
    fn rejects_duplicate_or_invalid_step_ids() {
        for (first_id, second_id) in [("same", "same"), ("1bad", "valid")] {
            let program = json!({
                "version": 1,
                "root": {
                    "op": "sequence",
                    "steps": [
                        {
                            "op": "call", "id": first_id, "tool": "read_file",
                            "arguments": { "path": "a.rs" }
                        },
                        {
                            "op": "call", "id": second_id, "tool": "read_file",
                            "arguments": { "path": "b.rs" }
                        },
                        { "op": "return", "value": null }
                    ]
                }
            });
            assert_eq!(
                validate_program_value(&program, &Catalog::standard(), &ProgramLimits::default())
                    .unwrap_err()
                    .kind,
                ProgramErrorKind::Validation
            );
        }
    }

    #[test]
    fn rejects_unknown_control_plane_and_recursive_tools() {
        for tool in ["unknown", "message", "run_tool_program", "call_sub_agent"] {
            let program = json!({
                "version": 1,
                "root": {
                    "op": "sequence",
                    "steps": [
                        { "op": "call", "id": "step", "tool": tool, "arguments": {} },
                        { "op": "return", "value": null }
                    ]
                }
            });
            assert_eq!(
                validate_program_value(&program, &Catalog::standard(), &ProgramLimits::default())
                    .unwrap_err()
                    .kind,
                ProgramErrorKind::PolicyDenied
            );
        }
    }

    #[test]
    fn rejects_non_parallel_capability_anywhere_in_parallel_subtree() {
        let program = json!({
            "version": 1,
            "root": {
                "op": "sequence",
                "steps": [
                    {
                        "op": "parallel",
                        "branches": [
                            {
                                "op": "call", "id": "read", "tool": "read_file",
                                "arguments": { "path": "a.rs" }
                            },
                            {
                                "op": "sequence",
                                "steps": [{
                                    "op": "call", "id": "write", "tool": "write_file",
                                    "arguments": { "path": "b.rs", "content": "x" }
                                }]
                            }
                        ]
                    },
                    { "op": "return", "value": null }
                ]
            }
        });

        let error =
            validate_program_value(&program, &Catalog::standard(), &ProgramLimits::default())
                .unwrap_err();
        assert_eq!(error.kind, ProgramErrorKind::PolicyDenied);
        assert_eq!(error.tool.as_deref(), Some("write_file"));
    }

    #[test]
    fn enforces_node_call_depth_parallel_and_input_budgets() {
        let catalog = Catalog::standard();

        let limits = ProgramLimits {
            max_nodes: 2,
            ..ProgramLimits::default()
        };
        assert_eq!(
            validate_program_value(&valid_program(), &catalog, &limits)
                .unwrap_err()
                .kind,
            ProgramErrorKind::LimitExceeded
        );

        let limits = ProgramLimits {
            max_calls: 2,
            ..ProgramLimits::default()
        };
        assert_eq!(
            validate_program_value(&valid_program(), &catalog, &limits)
                .unwrap_err()
                .kind,
            ProgramErrorKind::LimitExceeded
        );

        let limits = ProgramLimits {
            max_depth: 3,
            ..ProgramLimits::default()
        };
        assert_eq!(
            validate_program_value(&valid_program(), &catalog, &limits)
                .unwrap_err()
                .kind,
            ProgramErrorKind::LimitExceeded
        );

        let limits = ProgramLimits {
            max_parallel_branches: 2,
            ..ProgramLimits::default()
        };
        let three_branches = json!({
            "version": 1,
            "root": { "op": "sequence", "steps": [
                { "op": "parallel", "branches": [
                    { "op": "call", "id": "a", "tool": "read_file", "arguments": {} },
                    { "op": "call", "id": "b", "tool": "read_file", "arguments": {} },
                    { "op": "call", "id": "c", "tool": "read_file", "arguments": {} }
                ] },
                { "op": "return", "value": null }
            ] }
        });
        assert_eq!(
            validate_program_value(&three_branches, &catalog, &limits)
                .unwrap_err()
                .kind,
            ProgramErrorKind::LimitExceeded
        );

        let limits = ProgramLimits {
            max_input_bytes: 8,
            ..ProgramLimits::default()
        };
        assert_eq!(
            parse_and_validate_program(b"{}", &catalog, &limits)
                .unwrap_err()
                .kind,
            ProgramErrorKind::Parse
        );
        assert_eq!(
            parse_and_validate_program(b"{\"123456789\":true}", &catalog, &limits)
                .unwrap_err()
                .kind,
            ProgramErrorKind::LimitExceeded
        );
    }

    #[test]
    fn rejects_malformed_reference_during_static_validation() {
        let malformed = json!({
            "version": 1,
            "root": {
                "op": "sequence",
                "steps": [
                    {
                        "op": "call", "id": "read", "tool": "read_file",
                        "arguments": {
                            "path": {
                                "$ref": { "step": "prior" },
                                "fallback": "a.rs"
                            }
                        }
                    },
                    { "op": "return", "value": null }
                ]
            }
        });
        assert_eq!(
            validate_program_value(&malformed, &Catalog::standard(), &ProgramLimits::default())
                .unwrap_err()
                .kind,
            ProgramErrorKind::InvalidReference
        );
    }

    #[test]
    fn closure_can_supply_capability_catalog() {
        let catalog =
            |name: &str| (name == "read_file").then_some(CapabilityPolicy::parallel_readonly());
        let program = json!({
            "version": 1,
            "root": { "op": "sequence", "steps": [
                { "op": "call", "id": "read", "tool": "read_file", "arguments": {} },
                { "op": "return", "value": null }
            ] }
        });
        assert!(validate_program_value(&program, &catalog, &ProgramLimits::default()).is_ok());
    }
}

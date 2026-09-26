//! 工具程序 DSL 执行器：run_tool_program。
//! 数据面（可被程序调用的工具集合）由调用方（编排器工厂）注入。
//!
//! 模型只能看到这个工具的描述，看不到数据面工具自己的 schema。描述由
//! `guide` 按本轮数据面现写：程序形状、引用限制、每个授权工具的字段名。
//! 字面量参数在执行前按该 schema 校验；文本结果禁止 `/data/...` 子路径。
//! 子步骤直接 `execute`，不另建工具运行台账，文本按内联上限截断。

mod ast;
mod error;
mod executor;
mod guide;
mod managed;
mod support;
mod validate;
mod value;

use std::collections::HashMap;
use std::sync::Arc;

use rig::tool::{PortableDynamicTool, ToolExecutionError};
use tokio::sync::watch;

use self::error::{ProgramError, ProgramErrorKind};
use self::validate::CapabilityPolicy;
use super::deps::RigToolDeps;

/// 这些工具的程序结果是整段文本，`/data` 没有子字段。并行能力另见策略表
/// `supports_parallel_readonly`，不在这里抄一份。
const TEXT_RESULT_TOOLS: &[&str] = &[
    "read_file",
    "list_dir",
    "glob",
    "grep",
    "ssh_list_servers",
    "ssh_memo_read",
];

/// 描述数据面工具时用的快照。描述文本在工具构造时生成，不随单次调用变化。
pub(super) struct ToolContract {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub supports_parallel_readonly: bool,
    pub returns_text: bool,
}

/// 程序可调用的数据面：按名查找注入的 `PortableDynamicTool`。
///
/// 工具存在于数据面即视为已授权（对齐旧「runtime_capabilities ∩ 注册表」
/// 目录语义——授权裁剪由调用方组装数据面时完成）。
#[derive(Clone, Default)]
pub(crate) struct DataPlane {
    tools: Arc<HashMap<String, PortableDynamicTool>>,
    runtime: Option<RigToolDeps>,
}

impl DataPlane {
    /// 组装数据面，**不注入运行时依赖**（`runtime = None`）。
    ///
    /// runtime=None 时叶子走裸执行路径：`managed::execute` 直接
    /// `tool.execute(arguments)`，跳过受管叶子的登记、结算与命令门禁。
    /// 生产路径必须随后接 `with_runtime(deps)`（见 `program_tool`）；只有测试与
    /// 未接管 runtime 的外部调用方停在 None 形态。
    pub(crate) fn new(tools: Vec<PortableDynamicTool>) -> Self {
        let mut map = HashMap::with_capacity(tools.len());
        for tool in tools {
            if map.contains_key(tool.name()) {
                eprintln!(
                    "[agent] 警告：run_tool_program 数据面存在重名工具 '{}'，保留首个注册",
                    tool.name()
                );
                continue;
            }
            map.insert(tool.name().to_string(), tool);
        }
        Self {
            tools: Arc::new(map),
            runtime: None,
        }
    }

    /// 注入运行时依赖，把叶子切到受管路径（登记为内部工具运行 + 结算 + 门禁）。
    /// 生产入口 `program_tool` 恒走这里；`DataPlane::new` 之后未调用本方法即 runtime=None。
    fn with_runtime(mut self, deps: RigToolDeps) -> Self {
        self.runtime = Some(deps);
        self
    }

    pub(crate) fn get(&self, name: &str) -> Option<&PortableDynamicTool> {
        self.tools.get(name)
    }

    /// 静态校验用的能力目录条目。
    pub(crate) fn policy_for(&self, name: &str) -> Option<CapabilityPolicy> {
        self.get(name).map(|_| CapabilityPolicy {
            supports_parallel_readonly: super::spec::supports_parallel_readonly(name),
            returns_text: TEXT_RESULT_TOOLS.contains(&name),
        })
    }

    pub(super) fn contracts(&self) -> Vec<ToolContract> {
        let mut entries = self
            .tools
            .values()
            .map(|tool| {
                let definition = tool.definition();
                let name = definition.name;
                ToolContract {
                    supports_parallel_readonly: super::spec::supports_parallel_readonly(&name),
                    returns_text: TEXT_RESULT_TOOLS.contains(&name.as_str()),
                    description: definition.description,
                    parameters: definition.parameters,
                    name,
                }
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        entries
    }
}

/// `run_tool_program` 工具入口：数据面由调用方注入。
pub(crate) fn program_tool(
    deps: &RigToolDeps,
    data_plane: Vec<PortableDynamicTool>,
) -> PortableDynamicTool {
    build_program_tool(
        deps.cancel_rx.clone(),
        DataPlane::new(data_plane).with_runtime(deps.clone()),
    )
}

fn build_program_tool(
    cancel_rx: Option<watch::Receiver<bool>>,
    plane: DataPlane,
) -> PortableDynamicTool {
    let description = guide::render_description(&plane);
    PortableDynamicTool::new(
        "run_tool_program",
        description,
        ast::tool_program_parameters_schema(),
        move |args| {
            let plane = plane.clone();
            let cancel_rx =
                crate::agent::rig_ext::r#loop::invocation::ToolInvocationContext::current()
                    .map(|context| context.cancel_rx)
                    .or_else(|| cancel_rx.clone());
            Box::pin(async move {
                let limits = validate::ProgramLimits::default();
                let catalog = |name: &str| plane.policy_for(name);
                let program = validate::validate_program_value(&args, &catalog, &limits)
                    .map_err(program_error_tool_error)?;
                guide::reject_invalid_literal_arguments(&program, &plane)
                    .map_err(program_error_tool_error)?;
                executor::execute_program(&program, &plane, &limits, cancel_rx).await
            })
        },
    )
}

/// 将静态验证或运行期 ProgramError 映射为 rig 工具错误。
///
/// 模型可以改程序再试的失败都标成 retryable，避免和同批 `message` 一起被收口：
/// - Cancelled → `cancelled`；
/// - ChildFatal / Internal → `other` 且 retryable=false；
/// - DeadlineExceeded → `timeout`（默认可重试）；
/// - PolicyDenied → `permission_denied` 且 retryable（换成已授权工具即可）；
/// - Parse / Validation / LimitExceeded / InvalidReference → `invalid_args` 且 retryable；
/// - ChildRecoverable → `other` 且 retryable。
///
/// 步骤、位置和已完成步骤写进模型可见文本。rig 工具结果没有单独的 metadata 通道。
pub(crate) fn program_error_tool_error(error: ProgramError) -> ToolExecutionError {
    let kind = error.kind;
    let message = model_error_message(&error);
    let mapped = match kind {
        ProgramErrorKind::Cancelled => ToolExecutionError::cancelled(message),
        ProgramErrorKind::ChildFatal | ProgramErrorKind::Internal => {
            ToolExecutionError::other(message).with_retryable(false)
        }
        ProgramErrorKind::DeadlineExceeded => ToolExecutionError::timeout(message),
        ProgramErrorKind::PolicyDenied => {
            ToolExecutionError::permission_denied(message).with_retryable(true)
        }
        ProgramErrorKind::Parse
        | ProgramErrorKind::Validation
        | ProgramErrorKind::LimitExceeded
        | ProgramErrorKind::InvalidReference => {
            ToolExecutionError::invalid_args(message).with_retryable(true)
        }
        ProgramErrorKind::ChildRecoverable => {
            ToolExecutionError::other(message).with_retryable(true)
        }
    };
    match serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
    {
        Some(code) => mapped.with_code(code),
        None => mapped,
    }
}

fn model_error_message(error: &ProgramError) -> String {
    let mut message = format!("错误：ToolProgram 执行失败：{}", error.message);
    let mut extras = Vec::new();
    if let Some(step) = error.step_id.as_deref() {
        match error.tool.as_deref() {
            Some(tool) if !tool.is_empty() => extras.push(format!("步骤 {step}（{tool}）")),
            _ => extras.push(format!("步骤 {step}")),
        }
    }
    if let Some(path) = error.node_path.as_deref() {
        extras.push(format!("位置 {path}"));
    }
    if !extras.is_empty() {
        message.push(' ');
        message.push_str(&extras.join("，"));
        message.push('。');
    }
    if !error.completed_steps.is_empty() {
        message.push_str(&format!(
            " 已完成步骤：{}。这些步骤的输出没有随错误返回，修正后会重新执行。",
            error.completed_steps.join("、")
        ));
    }
    message
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::error::{ProgramError, ProgramErrorKind};
    use super::{build_program_tool, DataPlane};
    use rig::tool::{PortableDynamicTool, ToolErrorKind, ToolOutput};

    fn echo_tool() -> PortableDynamicTool {
        PortableDynamicTool::new("echo", "echo", json!({ "type": "object" }), |args| {
            Box::pin(async move { Ok(ToolOutput::json(args)) })
        })
    }

    #[test]
    fn data_plane_looks_up_by_name_and_marks_parallel_readonly() {
        let plane = DataPlane::new(vec![echo_tool()]);
        assert!(plane.get("echo").is_some());
        assert!(plane.get("missing").is_none());
        // echo 不在 PARALLEL_READONLY_TOOLS 表中：不可并行（fail-closed）。
        assert_eq!(
            plane.policy_for("echo"),
            Some(super::validate::CapabilityPolicy {
                supports_parallel_readonly: false,
                returns_text: false,
            })
        );
        assert_eq!(plane.policy_for("missing"), None);
    }

    #[test]
    fn data_plane_keeps_first_registration_on_duplicate_names() {
        let first = PortableDynamicTool::new("echo", "first", json!({}), |_| {
            Box::pin(async { Ok(ToolOutput::text("first")) })
        });
        let second = PortableDynamicTool::new("echo", "second", json!({}), |_| {
            Box::pin(async { Ok(ToolOutput::text("second")) })
        });
        let plane = DataPlane::new(vec![first, second]);
        assert_eq!(plane.get("echo").unwrap().definition().description, "first");
    }

    #[test]
    fn error_mapping_preserves_kind_code_and_retryability() {
        // 模型能改程序再试的失败（形状、参数、引用、策略、子调用）都是 retryable。
        // 取消、内部错误和子调用致命错误保持不可重试。
        let cases = [
            (
                ProgramErrorKind::Parse,
                ToolErrorKind::InvalidArgs,
                Some(true),
            ),
            (
                ProgramErrorKind::Validation,
                ToolErrorKind::InvalidArgs,
                Some(true),
            ),
            (
                ProgramErrorKind::LimitExceeded,
                ToolErrorKind::InvalidArgs,
                Some(true),
            ),
            (
                ProgramErrorKind::InvalidReference,
                ToolErrorKind::InvalidArgs,
                Some(true),
            ),
            (
                ProgramErrorKind::PolicyDenied,
                ToolErrorKind::PermissionDenied,
                Some(true),
            ),
            (
                ProgramErrorKind::ChildRecoverable,
                ToolErrorKind::Other,
                Some(true),
            ),
            (
                ProgramErrorKind::ChildFatal,
                ToolErrorKind::Other,
                Some(false),
            ),
            (
                ProgramErrorKind::Cancelled,
                ToolErrorKind::Cancelled,
                Some(false),
            ),
            (
                ProgramErrorKind::DeadlineExceeded,
                ToolErrorKind::Timeout,
                Some(true),
            ),
            (
                ProgramErrorKind::Internal,
                ToolErrorKind::Other,
                Some(false),
            ),
        ];
        for (program_kind, expected_kind, expected_retryable) in cases {
            let error = super::program_error_tool_error(ProgramError::new(program_kind, "boom"));
            assert_eq!(error.kind(), expected_kind, "{program_kind:?}");
            assert_eq!(error.retryable(), expected_retryable, "{program_kind:?}");
            assert_eq!(error.message(), "错误：ToolProgram 执行失败：boom");
            let code = serde_json::to_value(program_kind).unwrap();
            assert_eq!(error.code(), code.as_str());
        }
    }

    #[tokio::test]
    async fn program_tool_executes_end_to_end() {
        let tool = build_program_tool(None, DataPlane::new(vec![echo_tool()]));
        let program = json!({
            "version": 1,
            "root": { "op": "sequence", "steps": [
                { "op": "call", "id": "first", "tool": "echo", "arguments": { "value": "hello" } },
                { "op": "return", "value": { "$ref": { "step": "first", "pointer": "/data/value" } } }
            ] }
        });

        let output = tool.execute(program).await.expect("program succeeds");
        assert_eq!(output.as_text(), Some("hello"));
    }

    #[tokio::test]
    async fn program_tool_rejects_invalid_program_as_invalid_args() {
        let tool = build_program_tool(None, DataPlane::new(vec![echo_tool()]));
        let error = tool
            .execute(json!({ "version": 2, "root": {} }))
            .await
            .expect_err("invalid program rejected");
        // version=2 且 root={} 先被 serde 拒绝（Parse），而非版本校验（Validation）。
        assert_eq!(error.kind(), ToolErrorKind::InvalidArgs);
        assert_eq!(error.code(), Some("parse"));
        assert!(error.message().starts_with("错误：ToolProgram 执行失败："));
    }
}

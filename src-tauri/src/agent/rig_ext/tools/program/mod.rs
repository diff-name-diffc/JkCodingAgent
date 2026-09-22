//! 工具程序 DSL 执行器（T2.3b）：run_tool_program。
//! 数据面（可被程序调用的工具集合）由调用方（编排器工厂）注入。
//!
//! 迁移自旧 `agent/tools/builtin/run_tool_program.rs` + `agent/tools/program/`：
//! - 「按名调用工具」的接缝由旧 `CapabilityBroker` 改为注入的
//!   `Vec<PortableDynamicTool>` 数据面（按名查找 + `execute`）；
//! - schema（name / description / parameters）与旧实现逐字一致；
//! - 校验规则（`validate`）、模板引用解析（`value`）、执行器防御规则
//!   （并发/深度/预算/wall-time/drain，`executor` + `support`）逐条保留；
//! - 外层错误映射为带分类 code（ProgramErrorKind 的 snake_case 名）的
//!   `ToolExecutionError`，由 runtime 循环转成模型可见的「错误：…」文本。

mod ast;
mod error;
mod executor;
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

/// 工具描述：与旧 `builtin/run_tool_program.rs` 逐字一致（模型行为依赖文案，勿改写）。
const DESCRIPTION: &str = "在受限运行时中组合多个已授权工具调用。程序只支持 call、sequence、parallel、return；不执行 Python/JavaScript/Shell，不允许动态工具名。arguments 与 return.value 可用严格引用 {\"$ref\":{\"step\":\"步骤ID\",\"pointer\":\"/data/files\"}} 读取之前步骤的 JSON 结果。根节点必须是 sequence，且最后一步是全程序唯一 return。";

/// 可在 parallel 分支内执行的只读工具，迁移自旧 `tools/spec.rs`
/// TOOL_POLICY_TABLE 的 PARALLEL_READONLY 行（read_file / list_dir / glob /
/// grep / ssh_list_servers / ssh_memo_read）。`PortableDynamicTool` 不携带
/// access 元数据，校验器以本表为事实来源；未收录的工具一律按不可并行处理
/// （fail-closed，对齐旧 `ToolProfile::fail_closed` 的串行兜底）。
const PARALLEL_READONLY_TOOLS: &[&str] = &[
    "read_file",
    "list_dir",
    "glob",
    "grep",
    "ssh_list_servers",
    "ssh_memo_read",
];

/// 程序可调用的数据面：按名查找注入的 `PortableDynamicTool`。
///
/// 工具存在于数据面即视为已授权（对齐旧「runtime_capabilities ∩ 注册表」
/// 目录语义——授权裁剪由调用方组装数据面时完成）。
#[derive(Clone, Default)]
pub(crate) struct DataPlane {
    tools: Arc<HashMap<String, PortableDynamicTool>>,
}

impl DataPlane {
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
        }
    }

    pub(crate) fn get(&self, name: &str) -> Option<&PortableDynamicTool> {
        self.tools.get(name)
    }

    /// 静态校验用的能力目录条目。
    pub(crate) fn policy_for(&self, name: &str) -> Option<CapabilityPolicy> {
        self.get(name).map(|_| CapabilityPolicy {
            supports_parallel_readonly: PARALLEL_READONLY_TOOLS.contains(&name),
        })
    }
}

/// `run_tool_program` 工具入口：数据面由调用方注入。
pub(crate) fn program_tool(
    deps: &RigToolDeps,
    data_plane: Vec<PortableDynamicTool>,
) -> PortableDynamicTool {
    build_program_tool(deps.cancel_rx.clone(), DataPlane::new(data_plane))
}

fn build_program_tool(
    cancel_rx: Option<watch::Receiver<bool>>,
    plane: DataPlane,
) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "run_tool_program",
        DESCRIPTION,
        ast::tool_program_parameters_schema(),
        move |args| {
            let plane = plane.clone();
            let cancel_rx = cancel_rx.clone();
            Box::pin(async move {
                let limits = validate::ProgramLimits::default();
                let catalog = |name: &str| plane.policy_for(name);
                let program = validate::validate_program_value(&args, &catalog, &limits)
                    .map_err(program_error_tool_error)?;
                executor::execute_program(&program, &plane, &limits, cancel_rx).await
            })
        },
    )
}

/// 将静态验证或运行期 ProgramError 映射为 rig 工具错误。
///
/// 分类对齐旧 `program_error_result` 的 fatal/recoverable/cancelled 三态：
/// - Cancelled → `cancelled`（旧 `ToolResult::cancelled`）；
/// - ChildFatal / Internal → `other` 且 retryable=false（旧 fatal_error）；
/// - DeadlineExceeded → `timeout`（旧 recoverable，rig timeout 默认 retryable）；
/// - PolicyDenied → `permission_denied`（旧 recoverable，语义为策略拒绝）；
/// - Parse / Validation / LimitExceeded / InvalidReference → `invalid_args`
///   （旧 recoverable，模型可修正程序后重试）；
/// - ChildRecoverable → `other` 且 retryable=true（旧 recoverable）。
///
/// `code` 携带 ProgramErrorKind 的 snake_case 名，供策略层/审计机器可读；
/// 旧 `metadata.toolProgram`（version/completedSteps/error 详情）在 rig 工具
/// 结果模型中没有通道，由 Phase 3 策略层按需重建。
pub(crate) fn program_error_tool_error(error: ProgramError) -> ToolExecutionError {
    let kind = error.kind;
    let message = format!("错误：ToolProgram 执行失败：{}", error.message);
    let mapped = match kind {
        ProgramErrorKind::Cancelled => ToolExecutionError::cancelled(message),
        ProgramErrorKind::ChildFatal | ProgramErrorKind::Internal => {
            ToolExecutionError::other(message).with_retryable(false)
        }
        ProgramErrorKind::DeadlineExceeded => ToolExecutionError::timeout(message),
        ProgramErrorKind::PolicyDenied => ToolExecutionError::permission_denied(message),
        ProgramErrorKind::Parse
        | ProgramErrorKind::Validation
        | ProgramErrorKind::LimitExceeded
        | ProgramErrorKind::InvalidReference => ToolExecutionError::invalid_args(message),
        ProgramErrorKind::ChildRecoverable => {
            ToolExecutionError::other(message).with_retryable(true)
        }
    };
    match serde_json::to_value(kind).ok().and_then(|value| {
        value.as_str().map(str::to_string)
    }) {
        Some(code) => mapped.with_code(code),
        None => mapped,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::error::{ProgramError, ProgramErrorKind};
    use super::{build_program_tool, DataPlane};
    use rig::tool::{PortableDynamicTool, ToolErrorKind, ToolOutput};

    fn echo_tool() -> PortableDynamicTool {
        PortableDynamicTool::new(
            "echo",
            "echo",
            json!({ "type": "object" }),
            |args| Box::pin(async move { Ok(ToolOutput::json(args)) }),
        )
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
                supports_parallel_readonly: false
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
        // retryable 期望值即 rig 各类型的默认 retryability（result.rs 的
        // kind_defaults 表）叠加 with_retryable 覆盖后的结果。
        let cases = [
            (
                ProgramErrorKind::Parse,
                ToolErrorKind::InvalidArgs,
                Some(false),
            ),
            (
                ProgramErrorKind::Validation,
                ToolErrorKind::InvalidArgs,
                Some(false),
            ),
            (
                ProgramErrorKind::LimitExceeded,
                ToolErrorKind::InvalidArgs,
                Some(false),
            ),
            (
                ProgramErrorKind::InvalidReference,
                ToolErrorKind::InvalidArgs,
                Some(false),
            ),
            (
                ProgramErrorKind::PolicyDenied,
                ToolErrorKind::PermissionDenied,
                Some(false),
            ),
            (
                ProgramErrorKind::ChildRecoverable,
                ToolErrorKind::Other,
                Some(true),
            ),
            (ProgramErrorKind::ChildFatal, ToolErrorKind::Other, Some(false)),
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
            (ProgramErrorKind::Internal, ToolErrorKind::Other, Some(false)),
        ];
        for (program_kind, expected_kind, expected_retryable) in cases {
            let error =
                super::program_error_tool_error(ProgramError::new(program_kind, "boom"));
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

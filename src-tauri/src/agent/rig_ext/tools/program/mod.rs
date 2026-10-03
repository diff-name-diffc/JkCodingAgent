//! `run_tool_program`：进程内 JavaScript 编排本轮注入的数据面工具。
//!
//! 模型只看到这个入口。可调用的名字和字段在系统提示的「数据面 SDK」里，
//! 由同一份 `contracts()` 生成。单次绑定的参数错误是程序可捕获的
//! `ToolCallError`；只有未捕获的失败、无法执行的源码、超时或取消才让整次失败。

mod bindings;
mod dispatch;
mod error;
mod leaf_host;
mod limits;
mod managed;
mod runtime;
mod sdk;

use std::collections::HashMap;
use std::sync::Arc;

use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::Value;
use tauri::ipc::Channel;
use tokio::sync::watch;

use self::error::{CodeRunFailed, FailureKind};
use self::leaf_host::LeafHost;
use self::limits::ProgramLimits;
use super::deps::RigToolDeps;
use crate::agent::rig_ext::r#loop::invocation::ToolInvocationContext;

/// 描述数据面工具时用的快照。SDK 与绑定都从这里取名字、说明和参数 schema。
pub(super) struct ToolContract {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub supports_parallel_readonly: bool,
}

/// 程序可调用的数据面：按名查找注入的 `PortableDynamicTool`。
///
/// 工具存在于数据面即视为已授权。授权裁剪由调用方组装数据面时完成。
#[derive(Clone, Default)]
pub(crate) struct DataPlane {
    tools: Arc<HashMap<String, PortableDynamicTool>>,
    runtime: Option<RigToolDeps>,
    /// 叶子 `ToolRunUpdated` 的真实事件通道（见 `leaf_host.rs`）。生产入口注入；
    /// 裸路径（测试）保持 None。
    run_events: Option<Channel<crate::agent::rig_ext::events::AgentEvent>>,
}

impl DataPlane {
    /// 组装数据面，**不注入运行时依赖**（`runtime = None`）。
    ///
    /// runtime=None 时叶子走裸执行路径：`managed::execute` 直接
    /// `tool.execute(arguments)`，跳过受管叶子的登记、结算与命令门禁。
    /// 生产路径必须随后接 `with_runtime(deps)`（见 `program_tool`）。
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
            run_events: None,
        }
    }

    /// 注入运行时依赖，把叶子切到受管路径（登记为内部工具运行 + 结算 + 门禁）。
    fn with_runtime(mut self, deps: RigToolDeps) -> Self {
        self.runtime = Some(deps);
        self
    }

    /// 注入叶子台账事件的真实通道（当前 run 的事件通道）。
    fn with_run_events(
        mut self,
        events: Channel<crate::agent::rig_ext::events::AgentEvent>,
    ) -> Self {
        self.run_events = Some(events);
        self
    }

    pub(crate) fn get(&self, name: &str) -> Option<&PortableDynamicTool> {
        self.tools.get(name)
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

/// 本轮数据面的 SDK 文本。调用方必须用构造 `program_tool` 的同一批工具生成。
pub(crate) fn data_plane_sdk(tools: &[PortableDynamicTool]) -> String {
    let plane = DataPlane::new(tools.to_vec());
    sdk::render_section(&plane.contracts())
}

/// `run_tool_program` 工具入口：数据面与叶子事件通道由调用方注入。
pub(crate) fn program_tool(
    deps: &RigToolDeps,
    data_plane: Vec<PortableDynamicTool>,
    events: Channel<crate::agent::rig_ext::events::AgentEvent>,
) -> PortableDynamicTool {
    build_program_tool(
        deps.cancel_rx.clone(),
        DataPlane::new(data_plane)
            .with_runtime(deps.clone())
            .with_run_events(events),
    )
}

const PROGRAM_DESCRIPTION: &str = "\
用一段 JavaScript 异步函数体编排本轮数据面里的只读工具，完成一次调查。\
名字和字段只看系统提示中的「数据面 SDK」。返回这次调查要交给后续推理的结果。";

fn program_parameters_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["code", "description"],
        "properties": {
            "code": {
                "type": "string",
                "description": "异步函数体，不是完整脚本。用 await tools.名字(参数对象) 调用「数据面 SDK」中的工具，用 return 交回结果。没有文件、网络或进程。"
            },
            "description": {
                "type": "string",
                "description": "这次调查的标题，写清要查什么。"
            }
        }
    })
}

fn build_program_tool(
    cancel_rx: Option<watch::Receiver<bool>>,
    plane: DataPlane,
) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "run_tool_program",
        PROGRAM_DESCRIPTION,
        program_parameters_schema(),
        move |args| {
            let plane = plane.clone();
            let cancel_rx = cancel_rx.clone();
            Box::pin(async move { execute_program(plane, cancel_rx, args).await })
        },
    )
}

async fn execute_program(
    plane: DataPlane,
    cancel_rx: Option<watch::Receiver<bool>>,
    args: Value,
) -> Result<ToolOutput, ToolExecutionError> {
    let code = required_text(&args, "code")?;
    let _description = required_text(&args, "description")?;
    if let Some(name) = bindings::reserved_conflict(plane.tools.keys()) {
        return Err(program_error_tool_error(CodeRunFailed::new(
            FailureKind::Exception,
            format!("数据面占用了保留名字 '{name}'，本次程序拒绝启动"),
            "",
        )));
    }
    let parent = ToolInvocationContext::current();
    let cancel = parent
        .as_ref()
        .map(|context| context.cancel_rx.clone())
        .or(cancel_rx)
        .unwrap_or_else(|| watch::channel(false).1);
    let host = match (plane.runtime.as_ref(), parent.as_ref()) {
        (Some(deps), Some(context)) => Some(Arc::new(LeafHost::new(
            deps,
            context,
            plane.run_events.clone(),
        ))),
        _ => None,
    };
    // 实时挂载前提：前端卡片由 toolStarted 按 toolCallId 建立、没有 runId，而主路径
    // 调度器 run_events=None 不发根 run 的 ToolRunUpdated——不先补这一条，叶子的
    // parentRunId 在前端找不到父卡片，实时进度会被静默丢弃。失败只降级可见性，
    // 不影响程序执行。
    if let (Some(deps), Some(context), Some(run_events)) = (
        plane.runtime.as_ref(),
        parent.as_ref(),
        plane.run_events.as_ref(),
    ) {
        let (db, task) = (deps.db.clone(), context.task_id.clone());
        match tokio::task::spawn_blocking(move || db.load_tool_run(&task)).await {
            Ok(Ok(run)) => crate::agent::common::emit(
                run_events,
                crate::agent::rig_ext::events::AgentEvent::ToolRunUpdated {
                    run: Box::new(run),
                },
            ),
            Ok(Err(error)) => eprintln!(
                "[agent] run_tool_program 父运行记录读取失败，叶子实时事件可能无法挂载：{error:#}"
            ),
            Err(error) => eprintln!("[agent] run_tool_program 父运行记录读取任务失败：{error}"),
        }
    }
    let installed = bindings::install(&plane, host.clone(), parent);
    let outcome = runtime::run(code, installed, cancel, ProgramLimits::default()).await;
    if let Some(host) = host.as_ref() {
        if let Err(error) = host.shutdown().await {
            eprintln!("[agent] run_tool_program 叶子宿主收尾失败：{error:#}");
        }
    }
    outcome
        .map(|success| ToolOutput::text(render_success(&success)))
        .map_err(program_error_tool_error)
}

fn required_text<'a>(args: &'a Value, field: &str) -> Result<&'a str, ToolExecutionError> {
    match args.get(field).and_then(Value::as_str) {
        Some(text) if !text.trim().is_empty() => Ok(text),
        _ => Err(ToolExecutionError::invalid_args(format!(
            "错误：run_tool_program 需要非空字符串参数 {field}"
        ))
        .with_retryable(true)
        .with_code("invalid_arguments")),
    }
}

fn render_success(success: &runtime::ProgramSuccess) -> String {
    let body = match &success.value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| "null".to_string()),
    };
    if success.logs.is_empty() {
        body
    } else if body.is_empty() {
        success.logs.clone()
    } else {
        format!("{}\n{body}", success.logs)
    }
}

fn program_error_tool_error(error: CodeRunFailed) -> ToolExecutionError {
    let message = error.model_text();
    let mapped = match error.kind {
        FailureKind::Exception => ToolExecutionError::invalid_args(message).with_retryable(true),
        FailureKind::OutputLimit => ToolExecutionError::other(message).with_retryable(true),
        FailureKind::Timeout => ToolExecutionError::timeout(message).with_retryable(true),
        FailureKind::Abort => ToolExecutionError::cancelled(message),
    };
    mapped.with_code(error.kind.as_str())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use serde_json::json;

    use super::error::{CodeRunFailed, FailureKind};
    use super::{build_program_tool, program_error_tool_error, DataPlane};
    use rig::tool::{PortableDynamicTool, ToolErrorKind, ToolOutput};

    fn echo_tool() -> PortableDynamicTool {
        PortableDynamicTool::new(
            "echo",
            "echo",
            json!({ "type": "object", "additionalProperties": true }),
            |args| Box::pin(async move { Ok(ToolOutput::text(args.to_string())) }),
        )
    }

    #[test]
    fn data_plane_marks_registered_readonly_tools_parallel() {
        let plane = DataPlane::new(vec![
            PortableDynamicTool::new("read_file", "read", json!({}), |_| {
                Box::pin(async { Ok(ToolOutput::text("")) })
            }),
            echo_tool(),
        ]);
        let contracts = plane.contracts();
        let read = contracts
            .iter()
            .find(|entry| entry.name == "read_file")
            .unwrap();
        let echo = contracts.iter().find(|entry| entry.name == "echo").unwrap();
        assert!(read.supports_parallel_readonly);
        assert!(!echo.supports_parallel_readonly);
        assert!(plane.get("missing").is_none());
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
    fn error_mapping_uses_the_code_run_kinds() {
        let cases = [
            (
                FailureKind::Exception,
                ToolErrorKind::InvalidArgs,
                Some(true),
                "exception",
            ),
            (
                FailureKind::OutputLimit,
                ToolErrorKind::Other,
                Some(true),
                "output-limit",
            ),
            (
                FailureKind::Timeout,
                ToolErrorKind::Timeout,
                Some(true),
                "timeout",
            ),
            (
                FailureKind::Abort,
                ToolErrorKind::Cancelled,
                Some(false),
                "abort",
            ),
        ];
        for (kind, expected_kind, retryable, code) in cases {
            let error = program_error_tool_error(CodeRunFailed::new(kind, "boom", "seen"));
            assert_eq!(error.kind(), expected_kind, "{kind:?}");
            assert_eq!(error.retryable(), retryable, "{kind:?}");
            assert_eq!(error.code(), Some(code));
            let text = error.message();
            assert!(
                text.contains(&format!("code run failed ({code})")),
                "{text}"
            );
            assert!(text.contains("seen"), "{text}");
            assert!(!text.contains("重新执行"), "{text}");
        }
    }

    #[tokio::test]
    async fn missing_code_is_invalid_args_before_the_runtime() {
        let tool = build_program_tool(None, DataPlane::new(vec![echo_tool()]));
        let error = tool
            .execute(json!({ "description": "调查" }))
            .await
            .expect_err("missing code");
        assert_eq!(error.kind(), ToolErrorKind::InvalidArgs);
        assert_eq!(error.code(), Some("invalid_arguments"));
    }

    #[tokio::test]
    async fn reserved_binding_name_rejects_the_program_before_any_call() {
        let ran = Arc::new(AtomicUsize::new(0));
        let ran_tool = Arc::clone(&ran);
        let message = PortableDynamicTool::new(
            "message",
            "message",
            json!({ "type": "object" }),
            move |_| {
                ran_tool.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(ToolOutput::text("nope")) })
            },
        );
        let tool = build_program_tool(None, DataPlane::new(vec![message]));
        let error = tool
            .execute(json!({
                "code": "return await tools.message({});",
                "description": "不该启动"
            }))
            .await
            .expect_err("reserved name");
        assert_eq!(error.kind(), ToolErrorKind::InvalidArgs);
        assert!(error.message().contains("message"), "{}", error.message());
        assert_eq!(ran.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn program_tool_returns_the_javascript_result() {
        let tool = build_program_tool(None, DataPlane::new(vec![echo_tool()]));
        let output = tool
            .execute(json!({
                "code": "console.log('note'); return await tools.echo({ value: 'hello' });",
                "description": "回读 echo"
            }))
            .await
            .expect("program succeeds");
        let text = output.as_text().unwrap_or("");
        assert!(text.contains("note"), "{text}");
        assert!(text.contains("hello"), "{text}");
    }
}

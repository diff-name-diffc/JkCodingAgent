//! 数据面工具到 PTC 绑定的桥。运行时只看到 `call`，不读取工具名表以外的注册表。

use std::collections::BTreeMap;
use std::sync::Arc;

use futures::future::BoxFuture;
use rig::tool::{ToolErrorKind, ToolOutput};
use serde_json::Value;

use super::leaf_host::LeafHost;
use super::managed;
use super::DataPlane;
use crate::agent::rig_ext::r#loop::invocation::ToolInvocationContext;
use crate::agent::rig_ext::tool_result::clip_program_step_text;
use crate::agent::rig_ext::tools::run_record::prepare_arguments;

/// 这些名字属于编排器自己的协议面。数据面若占用它们，这次程序直接拒绝启动。
pub(super) const RESERVED_BINDING_NAMES: &[&str] = &[
    "run_tool_program",
    "message",
    "submit_graph",
    "graph_plan_report",
    "notify_user_progress",
    "call_sub_agent",
];

pub(super) enum BindingResult {
    Value(Value),
    ToolError(String),
    Cancelled,
}

pub(super) struct HostBinding {
    pub parallel: bool,
    pub call: Arc<dyn Fn(u64, Value) -> BoxFuture<'static, BindingResult> + Send + Sync>,
}

pub(super) fn reserved_conflict(
    names: impl IntoIterator<Item = impl AsRef<str>>,
) -> Option<String> {
    names
        .into_iter()
        .find(|name| RESERVED_BINDING_NAMES.contains(&name.as_ref()))
        .map(|name| name.as_ref().to_string())
}

/// 为当前数据面的每个工具建一个绑定。`parent` 和 `host` 缺席时走裸执行（测试）。
pub(super) fn install(
    plane: &DataPlane,
    host: Option<Arc<LeafHost>>,
    parent: Option<ToolInvocationContext>,
) -> BTreeMap<String, HostBinding> {
    let mut bindings = BTreeMap::new();
    for contract in plane.contracts() {
        let name = contract.name.clone();
        let parallel = contract.supports_parallel_readonly;
        let plane = plane.clone();
        let host = host.clone();
        let parent = parent.clone();
        let call_name = name.clone();
        bindings.insert(
            name,
            HostBinding {
                parallel,
                call: Arc::new(move |sequence, arguments| {
                    let plane = plane.clone();
                    let host = host.clone();
                    let parent = parent.clone();
                    let call_name = call_name.clone();
                    Box::pin(async move {
                        invoke(plane, host, parent, &call_name, sequence, arguments).await
                    })
                }),
            },
        );
    }
    bindings
}

async fn invoke(
    plane: DataPlane,
    host: Option<Arc<LeafHost>>,
    parent: Option<ToolInvocationContext>,
    name: &str,
    sequence: u64,
    arguments: Value,
) -> BindingResult {
    let Some(tool) = plane.get(name).cloned() else {
        return BindingResult::ToolError(format!("工具 '{name}' 不在本轮数据面"));
    };
    let schema = tool.definition().parameters;
    let arguments = match prepare_arguments(name, &schema, &arguments) {
        Ok(arguments) => arguments,
        Err(error) => return BindingResult::ToolError(strip_error_prefix(&error.message)),
    };
    let step = format!("ptc:{sequence}");
    let run = async {
        managed::execute(
            &plane,
            host.as_deref(),
            &tool,
            &step,
            sequence,
            arguments.clone(),
        )
        .await
    };
    let result = match parent {
        Some(parent) => parent.scope(run).await,
        None => run.await,
    };
    match result {
        Ok(output) => BindingResult::Value(canonical_value(name, &arguments, &output)),
        Err(error) if error.kind() == ToolErrorKind::Cancelled => BindingResult::Cancelled,
        Err(error) => BindingResult::ToolError(strip_error_prefix(error.message())),
    }
}

fn canonical_value(name: &str, arguments: &Value, output: &ToolOutput) -> Value {
    if let Some(payload) = output.as_json() {
        return payload.clone();
    }
    Value::String(clip_program_step_text(name, arguments, &output.render()))
}

fn strip_error_prefix(message: &str) -> String {
    message.trim_start_matches("错误：").trim().to_string()
}

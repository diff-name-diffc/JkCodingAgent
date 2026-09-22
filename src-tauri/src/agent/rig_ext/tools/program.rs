//! 工具程序 DSL 执行器（T2.3b）：run_tool_program。
//! 数据面（可被程序调用的工具集合）由调用方（编排器工厂）注入。

use rig::tool::PortableDynamicTool;

use super::deps::RigToolDeps;

pub(crate) fn program_tool(
    _deps: &RigToolDeps,
    _data_plane: Vec<PortableDynamicTool>,
) -> PortableDynamicTool {
    PortableDynamicTool::new("run_tool_program", "占位", serde_json::json!({"type": "object"}), |_args| {
        Box::pin(async move {
            Err(rig::tool::ToolExecutionError::refused("错误：未实现"))
        })
    })
}

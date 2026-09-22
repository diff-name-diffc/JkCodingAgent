//! MCP 动态工具桥（T2.4）：把 `mcp/` 注册表（Global/Project 作用域合并）
//! 枚举到的工具包装为 `PortableDynamicTool`（canonical 名 `mcp__<server>__<tool>`）。

use rig::tool::PortableDynamicTool;

use super::deps::RigToolDeps;

pub(crate) async fn mcp_tools(_deps: &RigToolDeps) -> Vec<PortableDynamicTool> {
    Vec::new()
}

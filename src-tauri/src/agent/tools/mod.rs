mod builtin;
mod context;
mod delegation;
mod mcp;
mod registry;

pub use context::ToolContext;
pub use delegation::{
    parse_continue_instruction, parse_dispatch_instruction, parse_exit_instruction, DispatchAgent,
};
pub use registry::ToolRegistry;

use crate::project::mcp::ProjectMcpRegistry;

impl ToolRegistry {
    pub fn default_tools(project_mcp_registry: ProjectMcpRegistry) -> Self {
        let mut tools = builtin::builtin_tools();
        tools.extend(delegation::delegation_tools());
        Self::new(tools).with_dynamic_provider(mcp::mcp_tool_bridge(project_mcp_registry))
    }
}

mod builtin;
mod context;
mod delegation;
mod mcp;
mod planning;
mod registry;

pub use context::ToolContext;
pub use delegation::{
    parse_continue_instruction, parse_dispatch_instruction, parse_exit_instruction, DispatchAgent,
};
pub use planning::{
    parse_ask_plan_question, parse_create_plan_document, parse_edit_plan_document,
    parse_present_plan, parse_replace_plan_document, parse_update_plan, UpdatePlanDraft,
};
pub use registry::{AgentTool, ToolRegistry};

use crate::project::mcp::ProjectMcpRegistry;

impl ToolRegistry {
    pub fn default_tools(project_mcp_registry: ProjectMcpRegistry) -> Self {
        let mut tools = builtin::builtin_tools();
        tools.extend(planning::planning_tools());
        tools.extend(delegation::delegation_tools());
        Self::new(tools).with_dynamic_provider(mcp::mcp_tool_bridge(project_mcp_registry))
    }

    pub fn plain_chat_tools() -> Self {
        Self::new(builtin::plain_chat_tools())
    }
}

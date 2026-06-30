mod builtin;
mod context;
mod delegation;
mod mcp;
mod planning;
pub mod provider;
mod registry;
mod result;
mod runtime;
mod spec;

pub use context::ToolContext;
pub use delegation::{
    parse_continue_instruction, parse_dispatch_instruction, parse_exit_instruction, DispatchAgent,
};
pub use planning::{
    parse_ask_plan_question, parse_create_plan_document, parse_edit_plan_document,
    parse_present_plan, parse_replace_plan_document, parse_update_plan, UpdatePlanDraft,
};
pub use registry::{AgentTool, ToolRegistry};
pub use result::{ToolAction, ToolInput, ToolResult, ToolStatus};
pub use runtime::{ToolRunFinishUpdate, ToolRuntime};
pub use spec::ToolSpec;

use crate::project::mcp::ProjectMcpRegistry;
use crate::ssh_tool::SshSessionManager;

impl ToolRegistry {
    pub fn default_tools(
        project_mcp_registry: ProjectMcpRegistry,
        ssh_manager: SshSessionManager,
    ) -> Self {
        let mut tools = builtin::builtin_tools(ssh_manager);
        tools.extend(planning::planning_tools());
        tools.extend(delegation::delegation_tools());
        tools.push(crate::agent::sub_agent::notify_user_progress_tool());
        Self::new(tools).with_dynamic_provider(mcp::mcp_tool_bridge(project_mcp_registry))
    }

    pub fn plain_chat_tools(
        project_mcp_registry: ProjectMcpRegistry,
        ssh_manager: SshSessionManager,
    ) -> Self {
        let mut tools = builtin::plain_chat_tools(ssh_manager);
        tools.push(crate::agent::sub_agent::notify_user_progress_tool());
        Self::new(tools).with_dynamic_provider(mcp::mcp_tool_bridge(project_mcp_registry))
    }
}

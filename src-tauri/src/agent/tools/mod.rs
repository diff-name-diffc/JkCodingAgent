mod builtin;
mod context;
mod mcp;
pub mod provider;
mod registry;
mod result;
mod runtime;
mod spec;

pub use context::ToolContext;
pub use registry::{AgentTool, ToolRegistry};
pub use result::{ToolAction, ToolInput, ToolResult, ToolStatus};
pub use runtime::{ToolRunFinishUpdate, ToolRuntime};
pub use spec::{ToolSafety, ToolSpec};

use crate::project::mcp::ProjectMcpRegistry;
use crate::ssh_tool::SshSessionManager;

impl ToolRegistry {
    pub fn default_tools(
        project_mcp_registry: ProjectMcpRegistry,
        ssh_manager: SshSessionManager,
    ) -> Self {
        let mut tools = builtin::builtin_tools(ssh_manager);
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

    /// 编排器专用注册表：固定只读工具 + submit_graph/graph_plan_report 协议壳。
    /// 不带 MCP 动态工具与写/执行类工具，从结构上保证项目 Agent 只读。
    /// （v3 清理：移除 list_sub_agents——编排器无 call_sub_agent，图节点也不允许
    /// subAgent 类型，只列不可调属于半吊子逻辑。）
    pub fn orchestrator_tools() -> Self {
        Self::new(builtin::orchestrator_tools())
    }
}

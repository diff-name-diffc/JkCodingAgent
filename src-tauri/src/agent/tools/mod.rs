mod broker;
mod builtin;
mod capability;
mod context;
mod mcp;
pub mod program;
mod registry;
mod result;
mod runtime;
mod spec;
mod surface;

pub use broker::{BrokerAudit, CapabilityBroker, CapabilityInvocation};
pub use capability::CapabilitySet;
pub use context::ToolContext;
pub use registry::{AgentTool, ToolRegistry};
pub(super) use result::ToolInput;
pub use result::{ToolAction, ToolResult, ToolStatus};
pub use runtime::{ToolRunFinishUpdate, ToolRuntime};
pub use spec::{ToolResultPolicy, ToolSafety, ToolSpec};
pub use surface::ToolSurface;

/// 项目编排器授权给受限运行时的数据面能力。协议/控制面工具不在用户可配置
/// 范围内，始终由编排器顶层显式处理。
pub const ORCHESTRATOR_RUNTIME_TOOL_NAMES: [&str; 4] = ["read_file", "list_dir", "glob", "grep"];
pub(crate) const MAX_TOOL_CALLS_PER_BATCH: usize = 32;
pub(crate) const MAX_PARALLEL_TOOL_CALLS: usize = 4;

use crate::mcp::McpRegistry;
use crate::ssh_tool::SshSessionManager;

impl ToolRegistry {
    pub fn default_tools(
        project_mcp_registry: McpRegistry,
        ssh_manager: SshSessionManager,
    ) -> Self {
        let mut tools = builtin::builtin_tools(ssh_manager);
        tools.push(crate::agent::sub_agent::notify_user_progress_tool());
        Self::new(tools).with_dynamic_provider(mcp::mcp_tool_bridge(project_mcp_registry))
    }

    pub fn plain_chat_tools(
        project_mcp_registry: McpRegistry,
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

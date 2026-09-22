//! rig 工具面与执行策略注入点。

use std::collections::HashMap;

use rig::message::ToolCall;
use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};

use super::super::tool_result::RigToolResultPolicy;

/// rig 工具面：可执行动态工具 + 每工具的结果策略（压缩阈值等）。
/// `definitions()` 产出请求用 `ToolDefinition`，执行按名查找。
pub struct RigToolSurface {
    tools: Vec<PortableDynamicTool>,
    policies: HashMap<String, RigToolResultPolicy>,
}

impl RigToolSurface {
    pub fn new(tools: Vec<PortableDynamicTool>) -> Self {
        Self {
            tools,
            policies: HashMap::new(),
        }
    }

    /// 挂载单个工具的结果策略（覆盖默认值）。
    pub fn with_policy(mut self, tool_name: impl Into<String>, policy: RigToolResultPolicy) -> Self {
        self.policies.insert(tool_name.into(), policy);
        self
    }

    pub fn definitions(&self) -> Vec<rig::completion::ToolDefinition> {
        self.tools.iter().map(|tool| tool.definition()).collect()
    }

    pub(super) fn find(&self, name: &str) -> Option<&PortableDynamicTool> {
        self.tools.iter().find(|tool| tool.name() == name)
    }

    pub(super) fn policy_for(&self, name: &str) -> RigToolResultPolicy {
        self.policies.get(name).copied().unwrap_or_default()
    }
}

/// 工具执行策略注入点（Phase 3 各 agent 的差异：审查门禁、路径规范化、
/// 协议拦截如 submit_graph）。默认实现 `DirectToolExecution` 直接执行。
#[async_trait::async_trait]
pub trait ToolExecutionPolicy: Send + Sync {
    async fn execute(
        &self,
        tool: &PortableDynamicTool,
        call: &ToolCall,
    ) -> std::result::Result<ToolOutput, ToolExecutionError>;
}

/// 默认策略：不加任何门禁，直接执行工具回调。
pub struct DirectToolExecution;

#[async_trait::async_trait]
impl ToolExecutionPolicy for DirectToolExecution {
    async fn execute(
        &self,
        tool: &PortableDynamicTool,
        call: &ToolCall,
    ) -> std::result::Result<ToolOutput, ToolExecutionError> {
        tool.execute(call.function.arguments.clone()).await
    }
}

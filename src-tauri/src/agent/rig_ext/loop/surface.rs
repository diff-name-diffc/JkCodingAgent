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
    /// 批量挂载结果策略（工具面装配期从策略表派生）。
    pub fn with_policies(
        mut self,
        policies: impl IntoIterator<Item = (String, RigToolResultPolicy)>,
    ) -> Self {
        self.policies.extend(policies);
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

/// `before_call` 的结果：可选拒绝理由（拒绝 = 不执行，直接以错误回灌）。
///
/// 台账不经此层：登记唯一走调度器 `enqueue` 的准入批次（`before_call` 复用
/// 准入产出的 `ToolInvocationContext`，不建裸台账行），终态唯一走 worker 的
/// `settle_tool_completion`——三段式的 `after_call` 台账收尾已随裸路径一并
/// 退役（P0-3 单写路径）。
pub struct ToolCallGuard {
    pub rejection: Option<ToolExecutionError>,
}

impl std::fmt::Debug for ToolCallGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolCallGuard")
            .field("rejected", &self.rejection.is_some())
            .finish()
    }
}

/// 工具执行策略注入点（各 agent 的差异：审查门禁、超时）。
///
/// 两段式：`before_call`（门禁：参数准备/取消/审查，可拒绝）→ `execute`
/// （实际执行，含统一超时）。trait 默认方法不加任何门禁，两段中只有
/// `execute` 有行为；生产策略由 `AppToolExecutionPolicy` 覆盖两段。
#[async_trait::async_trait]
pub trait ToolExecutionPolicy: Send + Sync {
    /// 宿主提供的真实工作区，模型参数不能改变资源域。
    fn resource_workspace(&self) -> Option<std::path::PathBuf> {
        None
    }

    fn registration_trace(&self) -> crate::agent::db::ToolRunTraceContext {
        Default::default()
    }

    /// 调用前置：门禁（审查/授权/参数准备）。
    async fn before_call(&self, _tool: &PortableDynamicTool, _call: &ToolCall) -> ToolCallGuard {
        ToolCallGuard { rejection: None }
    }

    /// 执行工具回调（默认直接执行）。
    async fn execute(
        &self,
        tool: &PortableDynamicTool,
        call: &ToolCall,
    ) -> std::result::Result<ToolOutput, ToolExecutionError> {
        tool.execute(call.function.arguments.clone()).await
    }
}

/// 默认策略：不加任何门禁、直接执行工具回调。
///
/// 仅测试使用——生产路径一律经 `AppToolExecutionPolicy`（审查门禁 + 统一超时），
/// 或由 trait 的默认方法提供等价行为。
#[cfg(test)]
#[derive(Clone)]
pub struct DirectToolExecution;

#[cfg(test)]
impl ToolExecutionPolicy for DirectToolExecution {}

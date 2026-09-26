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

/// 工具调用台账句柄（由策略层在 `before_call` 中创建，`after_call` 收尾）。
#[derive(Clone, Debug, Default)]
pub struct ToolCallTrace {
    /// `dispatcher_tool_runs` 行 id；None 表示本次调用未建台账
    /// （如未注册工具、台账创建失败）。
    pub run_id: Option<String>,
}

/// `before_call` 的结果：可选台账句柄 + 可选拒绝理由。
/// 拒绝时台账（若有）仍会被 `after_call` 收尾，不留悬挂记录。
pub struct ToolCallGuard {
    pub trace: Option<ToolCallTrace>,
    pub rejection: Option<ToolExecutionError>,
}

impl std::fmt::Debug for ToolCallGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolCallGuard")
            .field("trace", &self.trace)
            .field("rejected", &self.rejection.is_some())
            .finish()
    }
}

/// 一次工具调用的收尾输入（结果落库后由循环回传）。
pub struct ToolCallOutcome<'a> {
    pub status: &'a str,
    pub result_mode: Option<&'a str>,
    pub message_id: Option<&'a str>,
    pub error_kind: Option<&'a str>,
    pub error_message: Option<&'a str>,
    pub action_kind: Option<&'a str>,
}

/// 工具执行策略注入点（各 agent 的差异：审查门禁、台账、超时）。
///
/// 三段式：`before_call`（门禁 + 台账开始，可拒绝）→ `execute`（实际执行）
/// → `after_call`（结果落库后收尾台账）。trait 默认方法不加任何门禁，
/// 三段中只有 `execute` 有行为；生产策略由 `AppToolExecutionPolicy` 覆盖三段。
#[async_trait::async_trait]
pub trait ToolExecutionPolicy: Send + Sync {
    /// 宿主提供的真实工作区，模型参数不能改变资源域。
    fn resource_workspace(&self) -> Option<std::path::PathBuf> {
        None
    }

    fn registration_trace(&self) -> crate::agent::db::ToolRunTraceContext {
        Default::default()
    }

    /// 调用前置：门禁（审查/授权/参数准备）与台账开始。
    async fn before_call(&self, _tool: &PortableDynamicTool, _call: &ToolCall) -> ToolCallGuard {
        ToolCallGuard {
            trace: None,
            rejection: None,
        }
    }

    /// 执行工具回调（默认直接执行）。
    async fn execute(
        &self,
        tool: &PortableDynamicTool,
        call: &ToolCall,
    ) -> std::result::Result<ToolOutput, ToolExecutionError> {
        tool.execute(call.function.arguments.clone()).await
    }

    /// 结果落库后的收尾（台账终态）。失败仅告警，不影响主流程。
    async fn after_call(
        &self,
        _trace: Option<&ToolCallTrace>,
        _call: &ToolCall,
        _outcome: ToolCallOutcome<'_>,
    ) {
    }
}

/// 默认策略：不加任何门禁、不建台账，直接执行工具回调。
///
/// 仅测试使用——生产路径一律经 `AppToolExecutionPolicy`（审查门禁 + 台账），
/// 或由 trait 的默认方法提供等价行为。
#[cfg(test)]
#[derive(Clone)]
pub struct DirectToolExecution;

#[cfg(test)]
impl ToolExecutionPolicy for DirectToolExecution {}

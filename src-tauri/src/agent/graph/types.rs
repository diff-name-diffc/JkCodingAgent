//! PI 执行图 v3 的跨层数据契约。所有 serde 字段与前端保持 camelCase。
//!
//! v3 相对 v2 的方法论升级：闭环编排——执行图携带需求快照（requirement）与
//! 修复继承（inheritsFrom）；运行携带模式（full/resume）与验收结论（verdict）；
//! 节点携带预期文件（expectedFiles，供并行写冲突预检）与导出策略
//! （exportPolicy，控制下游注入摘要还是全文）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 执行图定义契约版本。单一事实来源：校验（validate）、编排器工具 schema
/// （tools/builtin/submit_graph）都引用本常量，避免再次升级时多处漂移。
pub(crate) const GRAPH_DEFINITION_VERSION: u8 = 3;

pub(crate) const PLAN_DRAFT: &str = "draft";
pub(crate) const PLAN_RUNNING: &str = "running";
pub(crate) const PLAN_COMPLETED: &str = "completed";
pub(crate) const PLAN_FAILED: &str = "failed";
pub(crate) const PLAN_CANCELLED: &str = "cancelled";

pub(crate) const NODE_PENDING: &str = "pending";
pub(crate) const NODE_RUNNING: &str = "running";
pub(crate) const NODE_SUCCEEDED: &str = "succeeded";
pub(crate) const NODE_FAILED: &str = "failed";
pub(crate) const NODE_SKIPPED: &str = "skipped";
pub(crate) const NODE_CANCELLED: &str = "cancelled";
pub(crate) const STATE_VALUE_MAX_CHARS: usize = 32_000;

/// 运行模式：full=完整执行；resume=断点续跑（复用上次运行的成功节点与 state）。
pub(crate) const RUN_MODE_FULL: &str = "full";
pub(crate) const RUN_MODE_RESUME: &str = "resume";

/// 验收结论：pass=产出满足需求；partial=有失败但有可用产出；fail=失败阻断或产出不符；
/// unknown=验收模型不可用/信息不足（此时回执仅罗列事实）。
pub(crate) const VERDICT_PASS: &str = "pass";
pub(crate) const VERDICT_PARTIAL: &str = "partial";
pub(crate) const VERDICT_FAIL: &str = "fail";
pub(crate) const VERDICT_UNKNOWN: &str = "unknown";

/// resume 复制的成功节点行使用的 phase 标记：回执与前端据此区分「本次执行」与「续跑复用」。
pub(crate) const NODE_PHASE_CACHED: &str = "cached";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphStateKey {
    pub key: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BaseToolGroup {
    ReadOnly,
    Coding,
}

impl BaseToolGroup {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Coding => "coding",
        }
    }
}

/// 节点输出对下游的导出策略。
/// - Summary（默认）：下游只注入输出中的「## 产出摘要」段（缺失时截取前 4000 字符），
///   控制深链条上下文膨胀；全文仍保留在 node_runs.output_text。
/// - Full：下游注入全文（32k 截断兜底），用于下游确实需要完整产出的场景。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExportPolicy {
    #[default]
    Summary,
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct GraphToolRef {
    pub source: String,
    pub name: String,
}

/// 修复图继承来源：新 plan 从既有 plan 的某次 run 继承共享 state（提交时快照种入）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphInherits {
    pub plan_id: String,
    pub run_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNode {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub role: String,
    pub model_ref: String,
    pub base_tool_group: BaseToolGroup,
    #[serde(default)]
    pub special_tools: Vec<GraphToolRef>,
    pub task: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub inject_state_keys: Vec<String>,
    pub output_key: String,
    /// 预期读写的文件（相对工作区），供并行写冲突预检；可选，未填不检测。
    #[serde(default)]
    pub expected_files: Vec<String>,
    /// 输出对下游的导出策略，默认摘要。
    #[serde(default)]
    pub export_policy: ExportPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphDefinition {
    pub version: u8,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub state_keys: Vec<GraphStateKey>,
    pub nodes: Vec<GraphNode>,
    /// 修复图继承来源；提交层校验合法性并把被继承 run 的 state 快照种入新 plan。
    #[serde(default)]
    pub inherits_from: Option<GraphInherits>,
}

impl GraphDefinition {
    /// 存量持久化定义的一次性升级：v2 → v3。
    /// v3 新增字段（expectedFiles/exportPolicy/inheritsFrom 等）均有 serde
    /// default，旧 v2 JSON 可完整反序列化，这里只需补版本号；升级后的定义
    /// 由 validate 正常放行（可重跑/续跑/编辑）。未知版本不动，由 validate 报错。
    /// 在 run_graph 与 graph_plan_update 入口调用，落库路径会把升级结果持久化。
    pub(crate) fn upgrade_legacy(&mut self) {
        if self.version == 2 {
            self.version = GRAPH_DEFINITION_VERSION;
        }
    }

    /// 统一 trim 节点 id、依赖引用、outputKey 与 injectStateKeys 的首尾空白。
    /// 校验/调度/持久化各处都以 trim 后的 id 为准（见 ReadyQueue 与 validate），
    /// 在解析入口统一规整可避免「原始 id 与 trim id 混用」导致的状态机错位。
    pub(crate) fn normalize_ids(&mut self) {
        for node in &mut self.nodes {
            node.id = node.id.trim().to_string();
            node.output_key = node.output_key.trim().to_string();
            for dep in &mut node.depends_on {
                *dep = dep.trim().to_string();
            }
            for key in &mut node.inject_state_keys {
                *key = key.trim().to_string();
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphRunSummary {
    pub id: String,
    pub plan_id: String,
    pub attempt_no: i64,
    pub status: String,
    /// full | resume。
    #[serde(default = "default_run_mode")]
    pub mode: String,
    /// pass | partial | fail | unknown。统一以 VERDICT_UNKNOWN 表示「尚未/未能验收」，
    /// 不再使用空串（读取层会把历史空串归一为 unknown，见 store::map_run）。
    #[serde(default = "default_verdict_status")]
    pub verdict_status: String,
    #[serde(default)]
    pub verdict_reason: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

fn default_run_mode() -> String {
    RUN_MODE_FULL.to_string()
}

fn default_verdict_status() -> String {
    VERDICT_UNKNOWN.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphPlanRecord {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub summary: String,
    pub definition_json: String,
    pub status: String,
    pub state_json: String,
    /// 提交时刻的用户需求快照：节点输入与终局验收都以它为准，避免运行前消息漂移。
    #[serde(default)]
    pub requirement: String,
    #[serde(default)]
    pub inherits_plan_id: Option<String>,
    #[serde(default)]
    pub inherits_run_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub latest_run_id: Option<String>,
    #[serde(default)]
    pub runs: Vec<GraphRunSummary>,
    #[serde(default)]
    pub node_runs: Vec<GraphNodeRunRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNodeRunRecord {
    pub run_id: String,
    pub plan_id: String,
    pub node_id: String,
    pub status: String,
    pub phase: String,
    pub model_ref: String,
    pub model_label: String,
    pub model_category: String,
    pub base_tool_group: String,
    pub special_tools_json: String,
    pub input_text: String,
    pub output_text: String,
    pub error_text: Option<String>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub usage_json: String,
    #[serde(default)]
    pub affected_files: Vec<String>,
    pub tool_call_count: i64,
    /// 已消耗的失败重试次数（重试时输入注入上次失败原因）。
    #[serde(default)]
    pub retry_count: i32,
}

impl GraphNodeRunRecord {
    pub(crate) fn pending(run_id: &str, plan_id: &str, node: &GraphNode) -> Self {
        Self {
            run_id: run_id.to_string(),
            plan_id: plan_id.to_string(),
            node_id: node.id.clone(),
            status: NODE_PENDING.to_string(),
            phase: "starting".to_string(),
            model_ref: node.model_ref.clone(),
            model_label: node.model_ref.clone(),
            model_category: String::new(),
            base_tool_group: node.base_tool_group.as_str().to_string(),
            special_tools_json: serde_json::to_string(&node.special_tools)
                .unwrap_or_else(|_| "[]".into()),
            input_text: String::new(),
            output_text: String::new(),
            error_text: None,
            started_at: None,
            finished_at: None,
            duration_ms: None,
            usage_json: "{}".to_string(),
            affected_files: Vec::new(),
            tool_call_count: 0,
            retry_count: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentActivity {
    pub id: String,
    pub run_id: String,
    pub node_id: String,
    pub sequence: i64,
    pub kind: String,
    pub status: String,
    pub title: String,
    pub content: String,
    pub payload_json: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphRunDetail {
    pub run: GraphRunSummary,
    pub node_runs: Vec<GraphNodeRunRecord>,
    pub activities: Vec<AgentActivity>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphHarnessModel {
    pub id: String,
    pub label: String,
    pub model: String,
    pub category: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphHarnessTool {
    pub source: String,
    pub name: String,
    pub description: String,
    pub provider: String,
    pub category: String,
    pub readonly: bool,
    pub review_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphHarnessCatalog {
    pub models: Vec<GraphHarnessModel>,
    pub tools: Vec<GraphHarnessTool>,
    #[serde(default)]
    pub diagnostics: Vec<String>,
}

/// 模型×基础工具组的历史运行统计（轻量学习回路：回注 Harness 目录辅助编排器选模型）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphModelStat {
    pub model_ref: String,
    pub base_tool_group: String,
    pub runs: i64,
    pub failures: i64,
    pub avg_duration_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphPlanUpdatedPayload {
    pub plan_id: String,
    pub workspace_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphRunEventPayload {
    pub plan_id: String,
    pub run_id: String,
    pub workspace_id: String,
    pub sequence: i64,
    pub timestamp_ms: i64,
    #[serde(flatten)]
    pub event: GraphRunEvent,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "event",
    content = "data",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GraphRunEvent {
    RunStarted {
        title: String,
        attempt_no: i64,
        node_count: usize,
    },
    NodeStarted {
        node_id: String,
        title: String,
        model_ref: String,
        model_label: String,
        input: String,
    },
    NodePhaseChanged {
        node_id: String,
        phase: String,
    },
    NodeOutputDelta {
        node_id: String,
        delta: String,
    },
    NodeActivity {
        node_id: String,
        activity: AgentActivity,
    },
    NodeFinished {
        node_id: String,
        output: String,
        duration_ms: u64,
        affected_files: Vec<String>,
    },
    NodeFailed {
        node_id: String,
        error: String,
        duration_ms: u64,
        affected_files: Vec<String>,
    },
    NodeSkipped {
        node_id: String,
        reason: String,
    },
    StateUpdated {
        node_id: String,
        key: String,
        value: String,
        state: Value,
    },
    /// 高危写检查点：就绪节点只剩 coding 节点且检查点未通过，运行暂停等待
    /// graph_run_resume。node_id 为触发暂停的 coding 节点；暂停期间已启动的
    /// 在途节点继续运行，就绪的非 coding 节点不受阻塞。
    RunPaused {
        node_id: String,
    },
    RunResumed {},
    RunFinished {
        state: Value,
        failed_nodes: Vec<String>,
        skipped_nodes: Vec<String>,
    },
    RunFailed {
        error: String,
    },
    RunCancelled {},
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v3_definition_round_trips() {
        let raw = r#"{"version":3,"title":"测试","inheritsFrom":{"planId":"p1","runId":"r1"},"nodes":[{"id":"n1","title":"实现","modelRef":"m1","baseToolGroup":"coding","specialTools":[],"task":"完成任务","outputKey":"result","expectedFiles":["src/a.rs"],"exportPolicy":"full"}]}"#;
        let definition: GraphDefinition = serde_json::from_str(raw).unwrap();
        assert_eq!(definition.version, 3);
        assert_eq!(definition.nodes[0].base_tool_group, BaseToolGroup::Coding);
        assert_eq!(definition.nodes[0].export_policy, ExportPolicy::Full);
        assert_eq!(definition.nodes[0].expected_files, vec!["src/a.rs"]);
        assert_eq!(
            definition.inherits_from.as_ref().unwrap().plan_id,
            "p1"
        );
    }

    #[test]
    fn v3_optional_fields_default() {
        let raw = r#"{"version":3,"title":"测试","nodes":[{"id":"n1","title":"实现","modelRef":"m1","baseToolGroup":"read_only","task":"调研","outputKey":"result"}]}"#;
        let definition: GraphDefinition = serde_json::from_str(raw).unwrap();
        let node = &definition.nodes[0];
        assert_eq!(node.export_policy, ExportPolicy::Summary);
        assert!(node.expected_files.is_empty());
        assert!(definition.inherits_from.is_none());
    }
}

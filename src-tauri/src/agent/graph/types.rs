//! PI 执行图 v2 的跨层数据契约。所有 serde 字段与前端保持 camelCase。

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const PLAN_DRAFT: &str = "draft";
pub(crate) const PLAN_CONFIRMED: &str = "confirmed";
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct GraphToolRef {
    pub source: String,
    pub name: String,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphRunSummary {
    pub id: String,
    pub plan_id: String,
    pub attempt_no: i64,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
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
    fn v2_definition_round_trips() {
        let raw = r#"{"version":2,"title":"测试","nodes":[{"id":"n1","title":"实现","modelRef":"m1","baseToolGroup":"coding","specialTools":[],"task":"完成任务","outputKey":"result"}]}"#;
        let definition: GraphDefinition = serde_json::from_str(raw).unwrap();
        assert_eq!(definition.version, 2);
        assert_eq!(definition.nodes[0].base_tool_group, BaseToolGroup::Coding);
    }
}

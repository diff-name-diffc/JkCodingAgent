//! 图编排（Graph Orchestrator）核心类型。
//!
//! - `GraphDefinition` / `GraphNode`：项目 Agent 的核心产物（执行图 DAG 定义）。
//! - `GraphPlanRecord` / `GraphNodeRunRecord`：`graph_plans` / `graph_node_runs` 表的记录映射。
//! - `GraphRunEventPayload` / `GraphRunEvent`：执行阶段全局广播的 `graph-run-event` 载荷。
//!
//! 所有结构 serde 一律 camelCase，与前端 `types.ts` 严格对齐。

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ─── 状态常量 ────────────────────────────────────────────────────────────────

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

/// 节点输出写回共享 state 前的截断上限（字符数）。
pub(crate) const STATE_VALUE_MAX_CHARS: usize = 32_000;

// ─── 图定义（项目 Agent 产物，JSON） ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphStateKey {
    pub key: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum GraphNodeAgent {
    SubAgent { agent_id: String },
    Claude,
    Codex,
}

impl GraphNodeAgent {
    pub(crate) fn kind_str(&self) -> &'static str {
        match self {
            Self::SubAgent { .. } => "subAgent",
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    pub(crate) fn agent_id(&self) -> Option<&str> {
        match self {
            Self::SubAgent { agent_id } => Some(agent_id.as_str()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNode {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub role: String,
    pub agent: GraphNodeAgent,
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
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub state_keys: Vec<GraphStateKey>,
    pub nodes: Vec<GraphNode>,
}

// ─── 持久化记录 ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphPlanRecord {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub summary: String,
    /// 图定义原文（GraphDefinition 的 JSON 字符串）。
    pub definition_json: String,
    pub status: String,
    /// 共享 state 最新快照（JSON 对象：key → 节点输出文本）。
    pub state_json: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// 关联的节点运行记录（非表字段，查询时装配）。
    #[serde(default)]
    pub node_runs: Vec<GraphNodeRunRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNodeRunRecord {
    pub plan_id: String,
    pub node_id: String,
    pub agent_kind: String,
    pub agent_id: Option<String>,
    pub status: String,
    pub input_text: String,
    pub output_text: String,
    pub error_text: Option<String>,
    /// subAgent 节点轨迹关联键（`graphnode:{plan_id}:{node_id}`）。
    pub trace_tool_call_id: Option<String>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub duration_ms: Option<i64>,
    /// 节点影响文件清单（节点执行前后 git status 快照差分采集；旧数据无此列时为空）。
    #[serde(default)]
    pub affected_files: Vec<String>,
}

impl GraphNodeRunRecord {
    pub(crate) fn pending(plan_id: &str, node: &GraphNode) -> Self {
        Self {
            plan_id: plan_id.to_string(),
            node_id: node.id.clone(),
            agent_kind: node.agent.kind_str().to_string(),
            agent_id: node.agent.agent_id().map(str::to_string),
            status: NODE_PENDING.to_string(),
            input_text: String::new(),
            output_text: String::new(),
            error_text: None,
            trace_tool_call_id: None,
            started_at: None,
            finished_at: None,
            duration_ms: None,
            affected_files: Vec::new(),
        }
    }
}

// ─── graph-plan-updated 事件载荷 ─────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphPlanUpdatedPayload {
    pub plan_id: String,
    pub workspace_id: String,
}

// ─── graph-run-event 事件载荷 ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphRunEventPayload {
    pub plan_id: String,
    pub workspace_id: String,
    pub timestamp_ms: i64,
    #[serde(flatten)]
    pub event: GraphRunEvent,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", content = "data", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum GraphRunEvent {
    RunStarted {
        title: String,
        node_count: usize,
    },
    NodeStarted {
        node_id: String,
        title: String,
        agent_kind: String,
        agent_id: Option<String>,
        input: String,
    },
    NodeOutputDelta {
        node_id: String,
        delta: String,
    },
    NodeFinished {
        node_id: String,
        output: String,
        duration_ms: u64,
        /// 节点影响文件（git status 快照差分）。
        affected_files: Vec<String>,
    },
    NodeFailed {
        node_id: String,
        error: String,
        duration_ms: u64,
        /// 节点影响文件（git status 快照差分；取消分支恒为空）。
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
    fn node_agent_serializes_with_kind_tag() {
        let sub = GraphNodeAgent::SubAgent {
            agent_id: "browser-agent".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&sub).unwrap(),
            serde_json::json!({ "kind": "subAgent", "agentId": "browser-agent" })
        );
        assert_eq!(
            serde_json::to_value(GraphNodeAgent::Claude).unwrap(),
            serde_json::json!({ "kind": "claude" })
        );
        assert_eq!(
            serde_json::to_value(GraphNodeAgent::Codex).unwrap(),
            serde_json::json!({ "kind": "codex" })
        );
    }

    #[test]
    fn node_agent_deserializes_camel_case() {
        let agent: GraphNodeAgent =
            serde_json::from_str(r#"{ "kind": "subAgent", "agentId": "a1" }"#).unwrap();
        assert_eq!(
            agent,
            GraphNodeAgent::SubAgent {
                agent_id: "a1".to_string()
            }
        );
    }

    #[test]
    fn run_event_payload_flattens_event_and_data() {
        let payload = GraphRunEventPayload {
            plan_id: "p1".to_string(),
            workspace_id: "w1".to_string(),
            timestamp_ms: 123,
            event: GraphRunEvent::NodeOutputDelta {
                node_id: "n1".to_string(),
                delta: "hello".to_string(),
            },
        };
        assert_eq!(
            serde_json::to_value(&payload).unwrap(),
            serde_json::json!({
                "planId": "p1",
                "workspaceId": "w1",
                "timestampMs": 123,
                "event": "nodeOutputDelta",
                "data": { "nodeId": "n1", "delta": "hello" }
            })
        );
    }

    #[test]
    fn definition_round_trips_camel_case() {
        let raw = r#"{
            "title": "重构认证模块",
            "summary": "先分析后改造",
            "stateKeys": [{ "key": "auth_analysis", "description": "现状分析" }],
            "nodes": [{
                "id": "n1",
                "title": "分析认证模块现状",
                "role": "代码分析专家",
                "agent": { "kind": "subAgent", "agentId": "browser-agent" },
                "task": "阅读 src/auth/**",
                "dependsOn": [],
                "injectStateKeys": [],
                "outputKey": "auth_analysis"
            }]
        }"#;
        let definition: GraphDefinition = serde_json::from_str(raw).unwrap();
        assert_eq!(definition.nodes.len(), 1);
        assert_eq!(definition.nodes[0].output_key, "auth_analysis");
        let serialized = serde_json::to_string(&definition).unwrap();
        assert!(serialized.contains("\"outputKey\":\"auth_analysis\""));
    }
}

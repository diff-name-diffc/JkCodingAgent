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

// ── 状态词表（集中定义，禁止散落魔法字符串）────────────────────────────
// 计划/运行状态（graph_plans.status 与 graph_runs.status 共用同一词表）。
// 注意：这些值同时被前端与 SQL 过滤条件引用，是落库/协议的稳定契约——
// 任何改动都等同于数据迁移，禁止调整既有值。
//
// 关于枚举化：本波次评估后维持「集中常量」方案，不引入状态 enum——
// 1) 落库值绝不能变：enum 序列化形态（tag/重命名）一旦与既有字符串值
//    漂移，存量用户数据（graph_plans/graph_runs/graph_node_runs）读取即错；
// 2) 读取侧是字符串世界：rusqlite 读出的 status/phase 是 String，SQL 过滤
//    条件与前端 normalize 都按字面值比较，enum 化需要全链路（store 读写、
//    serde 契约、前端类型）同步改造，收益不抵回归面；
// 3) phase 本身是开放集：sidecar lifecycle 事件会透传运行期阶段字符串
//    （不经下方固定表），phase 字段无法用封闭 enum 表达。
// 新增代码一律引用常量，不要再引入字面量；测试中的字面量是刻意保留的
// 落库值锁定（值变了测试必须红），不属于散落魔法字符串。
pub(crate) const PLAN_DRAFT: &str = "draft";
pub(crate) const PLAN_RUNNING: &str = "running";
pub(crate) const PLAN_COMPLETED: &str = "completed";
pub(crate) const PLAN_FAILED: &str = "failed";
pub(crate) const PLAN_CANCELLED: &str = "cancelled";

// 节点状态（graph_node_runs.status / 调度器事件）。
pub(crate) const NODE_PENDING: &str = "pending";
pub(crate) const NODE_RUNNING: &str = "running";
pub(crate) const NODE_SUCCEEDED: &str = "succeeded";
pub(crate) const NODE_FAILED: &str = "failed";
pub(crate) const NODE_SKIPPED: &str = "skipped";
pub(crate) const NODE_CANCELLED: &str = "cancelled";

/// 运行模式：full=完整执行；resume=断点续跑（复用上次运行的成功节点与 state）。
pub(crate) const RUN_MODE_FULL: &str = "full";
pub(crate) const RUN_MODE_RESUME: &str = "resume";

/// 验收结论：pass=产出满足需求；partial=有失败但有可用产出；fail=失败阻断或产出不符；
/// unknown=验收模型不可用/信息不足（此时回执仅罗列事实）。
pub(crate) const VERDICT_PASS: &str = "pass";
pub(crate) const VERDICT_PARTIAL: &str = "partial";
pub(crate) const VERDICT_FAIL: &str = "fail";
pub(crate) const VERDICT_UNKNOWN: &str = "unknown";

// 节点阶段（graph_node_runs.phase / NodePhaseChanged 事件）。
// sidecar 的 lifecycle 事件还会透传运行期阶段字符串（不经此表），
// 故 phase 字段保持 String；本表覆盖应用侧自行写入的固定阶段。
/// 节点行创建时的初始阶段（pending 快照）。
pub(crate) const NODE_PHASE_STARTING: &str = "starting";
/// 节点结算（成功/失败/跳过）落终态前的阶段。
pub(crate) const NODE_PHASE_FINALIZING: &str = "finalizing";
/// resume 复制的成功节点行使用的 phase 标记：回执与前端据此区分「本次执行」与「续跑复用」。
pub(crate) const NODE_PHASE_CACHED: &str = "cached";
/// 等待模型响应。
pub(crate) const NODE_PHASE_RESPONDING: &str = "responding";
/// 模型思考中。
pub(crate) const NODE_PHASE_THINKING: &str = "thinking";
/// 工具执行中。
pub(crate) const NODE_PHASE_TOOL_RUNNING: &str = "tool_running";
/// 失败自动重试中。
pub(crate) const NODE_PHASE_RETRYING: &str = "retrying";
/// 上下文压缩中。
pub(crate) const NODE_PHASE_COMPACTING: &str = "compacting";

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
    /// 要注入本节点输入的共享 state key 白名单（生产者必须是严格拓扑祖先）。
    /// 注入的是摘要值，单键与总量均有预算上限（见 input.rs）。
    #[serde(default)]
    pub inject_state_keys: Vec<String>,
    /// 本节点输出写回共享 state 的 key（写回的是「产出摘要」段，全文保留在
    /// node_runs.output_text）。
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
    /// 统一 trim 节点 id、依赖引用、outputKey、injectStateKeys 以及共享 state
    /// 定义侧 key（state_keys）的首尾空白。校验/调度/持久化各处都以 trim 后的
    /// id 为准（见 ReadyQueue 与 validate），在解析入口统一规整可避免「原始 id
    /// 与 trim id 混用」导致的状态机错位。injectStateKeys/outputKey 与
    /// state_keys.key 同属一个命名空间：只 trim 引用侧不 trim 定义侧，带空白
    /// 的定义键会匹配不上注入引用，造成运行期注入/写回错位。
    /// GraphInherits 的 planId/runId 参与精确匹配查询，一并归一化。
    pub(crate) fn normalize_ids(&mut self) {
        for key in &mut self.state_keys {
            key.key = key.key.trim().to_string();
        }
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
        if let Some(inherits) = &mut self.inherits_from {
            inherits.plan_id = inherits.plan_id.trim().to_string();
            inherits.run_id = inherits.run_id.trim().to_string();
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphRunSummary {
    pub id: String,
    pub plan_id: String,
    pub attempt_no: i64,
    /// 运行状态：取值见 PLAN_RUNNING / PLAN_COMPLETED / PLAN_FAILED /
    /// PLAN_CANCELLED（graph_runs 与 graph_plans 共用同一状态词表）。
    pub status: String,
    /// full | resume（RUN_MODE_FULL / RUN_MODE_RESUME）。
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
    /// 计划状态：PLAN_DRAFT / PLAN_RUNNING / PLAN_COMPLETED / PLAN_FAILED /
    /// PLAN_CANCELLED。命令层与 store 层的状态门禁都以此组常量为准。
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
    /// 节点状态：NODE_PENDING / NODE_RUNNING / NODE_SUCCEEDED / NODE_FAILED /
    /// NODE_SKIPPED / NODE_CANCELLED。
    pub status: String,
    /// 节点阶段：应用侧固定阶段见 NODE_PHASE_*（STARTING/FINALIZING/CACHED/
    /// RESPONDING/THINKING/TOOL_RUNNING/RETRYING/COMPACTING）；sidecar
    /// lifecycle 事件还会透传运行期阶段字符串，故保持 String。
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
            phase: NODE_PHASE_STARTING.to_string(),
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

/// graph-run-event 全局广播的载荷。
///
/// 线上 JSON 形态（由序列化测试 run_event_payload_flattens_event_into_top_level
/// 锁定，前端消费以此为准）：
/// ```json
/// {
///   "planId": "…", "runId": "…", "workspaceId": "…",
///   "sequence": 1, "timestampMs": 1700000000000,
///   "event": "nodePhaseChanged",
///   "data": { "nodeId": "…", "phase": "…" }
/// }
/// ```
/// `event`/`data` 两个键来自 adjacently tagged 的 GraphRunEvent 经
/// #[serde(flatten)] 内联，键名由 tag/content 固定、不受外层 rename_all
/// 影响；event 取值为变体名的 camelCase（rename_all_fields）。
/// 当前仅实现 Serialize：serde 对 flatten + adjacently tagged 的
/// Deserialize 支持有限，未来需要反序列化时必须自行实现，
/// 修改本结构前请先更新锁定测试并与前端对齐。
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
        /// 阶段字符串：应用侧固定阶段见 NODE_PHASE_*；sidecar lifecycle
        /// 事件透传的运行期阶段原样广播。
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
        /// 与 GraphNodeRunRecord.duration_ms 统一为 i64（毫秒），消除事件→
        /// 落库的跨层类型转换（u64→i64 溢出/符号歧义）。
        duration_ms: i64,
        affected_files: Vec<String>,
    },
    NodeFailed {
        node_id: String,
        error: String,
        /// 与 GraphNodeRunRecord.duration_ms 统一为 i64（毫秒）。
        duration_ms: i64,
        affected_files: Vec<String>,
    },
    NodeSkipped {
        node_id: String,
        reason: String,
    },
    /// 节点因运行取消而终止（落库状态 NODE_CANCELLED）：与 NodeSkipped
    /// （上游失败导致的传递性跳过）语义不同，前端据此区分「被取消」与
    /// 「被跳过」。仅在落库成功后广播（见 node_task::mark_node_cancelled）。
    NodeCancelled {
        node_id: String,
    },
    StateUpdated {
        node_id: String,
        key: String,
        value: String,
        state: Value,
    },
    /// 高危写检查点：就绪节点只剩「可能写盘」的节点（判定见
    /// runner::node_may_write——coding 工具组、可写特殊工具或 expectedFiles
    /// 任一即视为可写）且检查点未通过，运行暂停等待 graph_run_resume。
    /// node_id 为触发暂停的节点；暂停期间已启动的在途节点继续运行，
    /// 就绪的不可能写盘的节点不受阻塞。
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
        assert_eq!(definition.inherits_from.as_ref().unwrap().plan_id, "p1");
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

    #[test]
    fn normalize_ids_trims_state_keys_and_inherits() {
        let mut definition: GraphDefinition = serde_json::from_str(
            r#"{"version":3,"title":"测试","stateKeys":[{"key":" key1 ","description":"d"}],"inheritsFrom":{"planId":" p1 ","runId":" r1 "},"nodes":[{"id":" n1 ","title":"实现","modelRef":"m1","baseToolGroup":"coding","task":"调研","dependsOn":[" n0 "],"injectStateKeys":[" key1 "],"outputKey":" out "}]} "#,
        )
        .unwrap();
        definition.normalize_ids();
        assert_eq!(
            definition.state_keys[0].key, "key1",
            "定义侧 key 与引用侧同命名空间归一"
        );
        assert_eq!(definition.inherits_from.as_ref().unwrap().plan_id, "p1");
        assert_eq!(definition.inherits_from.as_ref().unwrap().run_id, "r1");
        let node = &definition.nodes[0];
        assert_eq!(node.id, "n1");
        assert_eq!(node.depends_on, vec!["n0"]);
        assert_eq!(node.inject_state_keys, vec!["key1"]);
        assert_eq!(node.output_key, "out");
    }

    #[test]
    fn run_event_payload_flattens_event_into_top_level() {
        // 锁定前端消费的扁平 JSON 形态：event/data 键来自 adjacently tagged
        // 枚举经 flatten 内联，不受外层 rename_all 影响。
        let payload = GraphRunEventPayload {
            plan_id: "p1".into(),
            run_id: "r1".into(),
            workspace_id: "w1".into(),
            sequence: 7,
            timestamp_ms: 123,
            event: GraphRunEvent::NodePhaseChanged {
                node_id: "n1".into(),
                phase: NODE_PHASE_THINKING.into(),
            },
        };
        let value = serde_json::to_value(&payload).unwrap();
        assert_eq!(value.get("planId").and_then(Value::as_str), Some("p1"));
        assert_eq!(value.get("timestampMs").and_then(Value::as_i64), Some(123));
        assert_eq!(
            value.get("event").and_then(Value::as_str),
            Some("nodePhaseChanged"),
            "变体名按 rename_all_fields 取 camelCase"
        );
        let data = value
            .get("data")
            .and_then(Value::as_object)
            .expect("data 为对象");
        assert_eq!(data.get("nodeId").and_then(Value::as_str), Some("n1"));
        assert_eq!(
            data.get("phase").and_then(Value::as_str),
            Some(NODE_PHASE_THINKING)
        );

        // 无字段变体：data 为空对象，event 键名稳定。
        let payload = GraphRunEventPayload {
            plan_id: "p1".into(),
            run_id: "r1".into(),
            workspace_id: "w1".into(),
            sequence: 8,
            timestamp_ms: 124,
            event: GraphRunEvent::RunCancelled {},
        };
        let value = serde_json::to_value(&payload).unwrap();
        assert_eq!(
            value.get("event").and_then(Value::as_str),
            Some("runCancelled")
        );
        assert_eq!(value.get("data"), Some(&serde_json::json!({})));

        // NodeCancelled：取消与跳过是不同语义，event 键名必须与
        // nodeSkipped 区分（前端按 event 判别节点终态展示）。
        let payload = GraphRunEventPayload {
            plan_id: "p1".into(),
            run_id: "r1".into(),
            workspace_id: "w1".into(),
            sequence: 9,
            timestamp_ms: 125,
            event: GraphRunEvent::NodeCancelled {
                node_id: "n1".into(),
            },
        };
        let value = serde_json::to_value(&payload).unwrap();
        assert_eq!(
            value.get("event").and_then(Value::as_str),
            Some("nodeCancelled")
        );
        let data = value
            .get("data")
            .and_then(Value::as_object)
            .expect("data 为对象");
        assert_eq!(data.get("nodeId").and_then(Value::as_str), Some("n1"));
    }
}

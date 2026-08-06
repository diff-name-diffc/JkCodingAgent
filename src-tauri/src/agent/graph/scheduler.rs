//! 依赖驱动的 ready-queue 调度状态机（纯函数：无 async / IO / tauri 依赖，可独立单测）。
//!
//! 取代 v2 的层屏障调度：节点完成即解锁下游，不必等同层其余节点。
//! 异步驱动层（runner）从 `ready_nodes()` 取至多 `MAX_PARALLEL_NODES` 个节点并发执行，
//! 结束后用 `on_finished` 推进状态机；失败节点由驱动层结合 `retryable` 决定重试一次。

use std::collections::{HashMap, HashSet, VecDeque};

use super::types::GraphDefinition;

/// 同一 run 内的最大并发节点数。
pub(crate) const MAX_PARALLEL_NODES: usize = 3;
/// 每个节点最多重试次数（重试时输入注入上次失败原因）。
pub(crate) const MAX_NODE_RETRIES: i32 = 1;

/// 节点结算结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinishKind {
    Succeeded,
    FailedFinal,
}

/// 调度器视角的节点状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NodeState {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
    Cancelled,
}

impl NodeState {
    fn settled(&self) -> bool {
        !matches!(self, NodeState::Pending | NodeState::Running)
    }
}

pub(crate) struct ReadyQueue {
    node_ids: Vec<String>,
    status: HashMap<String, NodeState>,
    retry_count: HashMap<String, i32>,
    dependencies: HashMap<String, Vec<String>>,
    dependents: HashMap<String, Vec<String>>,
    in_flight: HashSet<String>,
}

impl ReadyQueue {
    /// `initial_status` 来自 node_runs 记录（status 字符串）：full 模式全部 pending；
    /// resume 模式含 succeeded（cached）节点，调度器不会重跑它们。
    /// 注意 runner 当前数据流下 initial_status 只可能含 succeeded/pending
    /// （create_resume_run 仅复制 succeeded 行，其余节点重写 pending 重跑）；
    /// failed/cancelled/skipped 分支为防御性保留（见 cascade_initial_terminal）。
    pub(crate) fn new(definition: &GraphDefinition, initial_status: &HashMap<String, String>) -> Self {
        let mut status = HashMap::new();
        for node in &definition.nodes {
            let id = node.id.trim().to_string();
            let state = match initial_status.get(&id).map(String::as_str) {
                Some("succeeded") => NodeState::Succeeded,
                Some("failed") => NodeState::Failed,
                Some("skipped") => NodeState::Skipped,
                Some("cancelled") => NodeState::Cancelled,
                _ => NodeState::Pending,
            };
            status.insert(id, state);
        }
        let mut dependencies: HashMap<String, Vec<String>> = HashMap::new();
        let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
        for node in &definition.nodes {
            let id = node.id.trim().to_string();
            let deps = node
                .depends_on
                .iter()
                .map(|dep| dep.trim().to_string())
                .collect::<Vec<_>>();
            for dep in &deps {
                dependents.entry(dep.clone()).or_default().push(id.clone());
            }
            dependencies.insert(id, deps);
        }
        Self {
            node_ids: definition.nodes.iter().map(|n| n.id.trim().to_string()).collect(),
            status,
            retry_count: HashMap::new(),
            dependencies,
            dependents,
            in_flight: HashSet::new(),
        }
    }

    /// 初始状态里 failed/cancelled/skipped 节点的下游必然无法运行（依赖需全部
    /// 成功）：构造后立刻级联 skip，与 on_finished(FailedFinal) 的语义对齐。
    /// 否则这些下游会永远停留在 Pending（ready_nodes 恒为空），收尾只能依赖
    /// 驱动层的防御性 cancel_remaining 兜底，且状态语义不准（应为 skipped
    /// 而非 cancelled）。注意 Skipped 同为终态（new() 会把 "skipped" 映射为
    /// NodeState::Skipped），必须与 Failed/Cancelled 一并纳入级联触发集合，
    /// 否则初始 skipped 节点的下游永久 Pending、调度无法收尾。
    /// 当前 runner 数据流不会触发本方法（resume 仅复制 succeeded 行，见
    /// ReadyQueue::new 注释）；作为未来可能出现终态初始状态的防御性兜底。
    /// 返回新跳过的节点，由驱动层落库并发事件。
    pub(crate) fn cascade_initial_terminal(&mut self) -> Vec<String> {
        let terminal: Vec<String> = self
            .node_ids
            .iter()
            .filter(|id| {
                matches!(
                    self.status.get(*id),
                    Some(NodeState::Failed)
                        | Some(NodeState::Cancelled)
                        | Some(NodeState::Skipped)
                )
            })
            .cloned()
            .collect();
        let mut skipped = Vec::new();
        for id in terminal {
            skipped.extend(self.cascade_skip(&id));
        }
        skipped.sort();
        skipped.dedup();
        skipped
    }

    /// 可执行节点：状态 pending、未在执行中、全部依赖已 succeeded。按 id 排序保证确定性。
    pub(crate) fn ready_nodes(&self) -> Vec<String> {
        let mut ready = self
            .node_ids
            .iter()
            .filter(|id| {
                self.status.get(*id) == Some(&NodeState::Pending)
                    && !self.in_flight.contains(*id)
                    && self
                        .dependencies
                        .get(*id)
                        .map(|deps| {
                            deps.iter()
                                .all(|dep| self.status.get(dep) == Some(&NodeState::Succeeded))
                        })
                        .unwrap_or(true)
            })
            .cloned()
            .collect::<Vec<_>>();
        ready.sort();
        ready
    }

    /// 驱动层取走节点投入执行。
    pub(crate) fn claim(&mut self, node_id: &str) {
        self.status.insert(node_id.to_string(), NodeState::Running);
        self.in_flight.insert(node_id.to_string());
    }

    /// 节点执行结束，推进状态机。FailedFinal 会传递性地把注定无法运行的下游标记为
    /// skipped 并返回（驱动层据此落库/发事件）；Succeeded 解锁下游由 ready_nodes 体现。
    ///
    /// 仅当节点当前为 Running 时才推进：若驱动层已通过其他路径结算该节点
    /// （如 cancel_remaining 把在途节点标为 Cancelled）后又把在途任务的迟到结果
    /// 交到这里，不能让终态被覆盖回 Succeeded/Failed，否则与已发出的事件及 DB
    /// 记录自相矛盾。
    pub(crate) fn on_finished(&mut self, node_id: &str, kind: FinishKind) -> Vec<String> {
        self.in_flight.remove(node_id);
        if self.status.get(node_id) != Some(&NodeState::Running) {
            return Vec::new();
        }
        match kind {
            FinishKind::Succeeded => {
                self.status.insert(node_id.to_string(), NodeState::Succeeded);
                Vec::new()
            }
            FinishKind::FailedFinal => {
                self.status.insert(node_id.to_string(), NodeState::Failed);
                self.cascade_skip(node_id)
            }
        }
    }

    /// 失败节点的下游必然无法运行（依赖需全部成功）：BFS 传递标记 skipped。
    fn cascade_skip(&mut self, failed_id: &str) -> Vec<String> {
        let mut skipped = Vec::new();
        let mut queue: VecDeque<String> = self
            .dependents
            .get(failed_id)
            .cloned()
            .unwrap_or_default()
            .into();
        let mut enqueued: HashSet<String> = queue.iter().cloned().collect();
        while let Some(id) = queue.pop_front() {
            if self.status.get(&id).map(NodeState::settled).unwrap_or(true) {
                continue;
            }
            self.status.insert(id.clone(), NodeState::Skipped);
            skipped.push(id.clone());
            for child in self.dependents.get(&id).cloned().unwrap_or_default() {
                if enqueued.insert(child.clone()) {
                    queue.push_back(child);
                }
            }
        }
        skipped.sort();
        skipped
    }

    /// 取消时把所有未结算节点标记为 cancelled 并返回（驱动层落库/发事件）。
    pub(crate) fn cancel_remaining(&mut self) -> Vec<String> {
        let mut cancelled = Vec::new();
        for id in &self.node_ids {
            if !self.status.get(id).map(NodeState::settled).unwrap_or(true) {
                self.status.insert(id.clone(), NodeState::Cancelled);
                cancelled.push(id.clone());
            }
        }
        self.in_flight.clear();
        cancelled.sort();
        cancelled
    }

    pub(crate) fn retryable(&self, node_id: &str) -> bool {
        self.retry_count.get(node_id).copied().unwrap_or(0) < MAX_NODE_RETRIES
    }

    /// 记录一次重试消耗（驱动层在重入队前调用）。
    ///
    /// 仅当节点当前为 Running 时才接受重试：与 on_finished 相同的守卫，
    /// 防御驱动层已通过其他路径结算该节点（如 cancel_remaining 把在途节点
    /// 标为 Cancelled）后，迟到的失败结果把终态复活回 Pending——那会让
    /// has_unsettled 恒真、与已发出的取消事件及 DB 记录矛盾。
    /// 返回 false 表示未接受重试，驱动层应按最终失败（on_finished）处理。
    pub(crate) fn record_retry(&mut self, node_id: &str) -> bool {
        self.in_flight.remove(node_id);
        if self.status.get(node_id) != Some(&NodeState::Running) {
            return false;
        }
        *self.retry_count.entry(node_id.to_string()).or_insert(0) += 1;
        // 节点回到 pending 等待重入队；claim 时会再置 running。
        self.status.insert(node_id.to_string(), NodeState::Pending);
        true
    }

    pub(crate) fn retry_count(&self, node_id: &str) -> i32 {
        self.retry_count.get(node_id).copied().unwrap_or(0)
    }

    pub(crate) fn is_settled(&self, node_id: &str) -> bool {
        self.status.get(node_id).map(NodeState::settled).unwrap_or(true)
    }

    /// 是否还有未结算节点。驱动层据此判断收尾与防御性清理。
    pub(crate) fn has_unsettled(&self) -> bool {
        self.node_ids.iter().any(|id| !self.is_settled(id))
    }

    pub(crate) fn failed_nodes(&self) -> Vec<String> {
        let mut nodes: Vec<String> = self
            .node_ids
            .iter()
            .filter(|id| self.status.get(*id) == Some(&NodeState::Failed))
            .cloned()
            .collect();
        nodes.sort();
        nodes
    }

    pub(crate) fn skipped_nodes(&self) -> Vec<String> {
        let mut nodes: Vec<String> = self
            .node_ids
            .iter()
            .filter(|id| self.status.get(*id) == Some(&NodeState::Skipped))
            .cloned()
            .collect();
        nodes.sort();
        nodes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::graph::types::{BaseToolGroup, GraphNode};

    fn node(id: &str, deps: &[&str]) -> GraphNode {
        GraphNode {
            id: id.into(),
            title: id.into(),
            role: String::new(),
            model_ref: "m1".into(),
            base_tool_group: BaseToolGroup::Coding,
            special_tools: vec![],
            task: "task".into(),
            depends_on: deps.iter().map(|v| v.to_string()).collect(),
            inject_state_keys: vec![],
            output_key: format!("out_{id}"),
            expected_files: vec![],
            export_policy: Default::default(),
        }
    }
    fn definition(nodes: Vec<GraphNode>) -> GraphDefinition {
        GraphDefinition {
            version: 3,
            title: "test".into(),
            summary: String::new(),
            state_keys: vec![],
            nodes,
            inherits_from: None,
        }
    }

    #[test]
    fn unlocks_downstream_on_success_without_layer_barrier() {
        // a→c、b→c 且 b 与 a 同层：a 成功后即使 b 未完成，a 的其他下游也应解锁。
        // 这里用 a→c、b 独立验证「无需等全层」。
        let def = definition(vec![node("a", &[]), node("b", &[]), node("c", &["a"])]);
        let mut queue = ReadyQueue::new(&def, &HashMap::new());
        assert_eq!(queue.ready_nodes(), vec!["a", "b"]);

        queue.claim("a");
        queue.claim("b");
        let newly_skipped = queue.on_finished("a", FinishKind::Succeeded);
        assert!(newly_skipped.is_empty());
        // a 完成即解锁 c，尽管 b 仍在执行。
        assert!(queue.ready_nodes().contains(&"c".to_string()));
    }

    #[test]
    fn failure_blocks_transitive_downstream() {
        let def = definition(vec![node("a", &[]), node("b", &["a"]), node("c", &["b"])]);
        let mut queue = ReadyQueue::new(&def, &HashMap::new());
        queue.claim("a");
        let skipped = queue.on_finished("a", FinishKind::FailedFinal);
        assert_eq!(skipped, vec!["b", "c"]);
        assert!(queue.ready_nodes().is_empty());
        assert_eq!(queue.failed_nodes(), vec!["a"]);
        assert_eq!(queue.skipped_nodes(), vec!["b", "c"]);
        assert!(!queue.has_unsettled());
    }

    #[test]
    fn independent_branch_survives_other_branch_failure() {
        let def = definition(vec![node("a", &[]), node("b", &[]), node("c", &["a"])]);
        let mut queue = ReadyQueue::new(&def, &HashMap::new());
        queue.claim("a");
        let skipped = queue.on_finished("a", FinishKind::FailedFinal);
        assert_eq!(skipped, vec!["c"]);
        // b 不受影响。
        assert_eq!(queue.ready_nodes(), vec!["b"]);
    }

    #[test]
    fn retry_once_then_fail_final() {
        let def = definition(vec![node("a", &[]), node("b", &["a"])]);
        let mut queue = ReadyQueue::new(&def, &HashMap::new());
        queue.claim("a");
        assert!(queue.retryable("a"));
        assert!(queue.record_retry("a"));
        assert_eq!(queue.retry_count("a"), 1);
        // 重试后回到就绪队列。
        assert_eq!(queue.ready_nodes(), vec!["a"]);
        queue.claim("a");
        assert!(!queue.retryable("a"), "最多重试一次");
        let skipped = queue.on_finished("a", FinishKind::FailedFinal);
        assert_eq!(skipped, vec!["b"]);
    }

    #[test]
    fn record_retry_refuses_to_resurrect_settled_node() {
        // 与 on_finished 的守卫对称：cancel_remaining 已把在途节点标为
        // Cancelled 后，迟到的失败结果不能借重试把终态复活回 Pending。
        let def = definition(vec![node("a", &[]), node("b", &["a"])]);
        let mut queue = ReadyQueue::new(&def, &HashMap::new());
        queue.claim("a");
        queue.cancel_remaining();
        assert!(!queue.record_retry("a"));
        assert_eq!(queue.retry_count("a"), 0, "拒绝重试时不应消耗重试次数");
        assert!(queue.ready_nodes().is_empty());
        assert!(!queue.has_unsettled(), "a/b 均保持 Cancelled，可正常收尾");
    }

    #[test]
    fn resume_initial_succeeded_nodes_are_not_rerun() {
        let def = definition(vec![node("a", &[]), node("b", &["a"])]);
        let mut initial = HashMap::new();
        initial.insert("a".to_string(), "succeeded".to_string());
        let mut queue = ReadyQueue::new(&def, &initial);
        assert_eq!(queue.ready_nodes(), vec!["b"], "a 已缓存，只跑 b");
        queue.claim("b");
        queue.on_finished("b", FinishKind::Succeeded);
        assert!(!queue.has_unsettled());
        assert!(queue.failed_nodes().is_empty());
    }

    #[test]
    fn cancel_remaining_marks_unsettled_nodes() {
        let def = definition(vec![node("a", &[]), node("b", &["a"])]);
        let mut queue = ReadyQueue::new(&def, &HashMap::new());
        queue.claim("a");
        let cancelled = queue.cancel_remaining();
        assert_eq!(cancelled, vec!["a", "b"]);
        assert!(!queue.has_unsettled());
    }

    #[test]
    fn initial_failed_node_cascades_skip_to_downstream() {
        // resume 基线含失败节点时，其下游应在构造后被级联 skip，
        // 而不是永远 pending 导致调度器无法收尾。
        let def = definition(vec![node("a", &[]), node("b", &["a"]), node("c", &["b"])]);
        let mut initial = HashMap::new();
        initial.insert("a".to_string(), "failed".to_string());
        let mut queue = ReadyQueue::new(&def, &initial);
        let skipped = queue.cascade_initial_terminal();
        assert_eq!(skipped, vec!["b", "c"]);
        assert!(queue.ready_nodes().is_empty());
        assert!(!queue.has_unsettled());
    }

    #[test]
    fn initial_cancelled_node_also_cascades_skip() {
        let def = definition(vec![node("a", &[]), node("b", &["a"]), node("d", &[])]);
        let mut initial = HashMap::new();
        initial.insert("a".to_string(), "cancelled".to_string());
        let mut queue = ReadyQueue::new(&def, &initial);
        let skipped = queue.cascade_initial_terminal();
        assert_eq!(skipped, vec!["b"]);
        // 独立分支不受影响。
        assert_eq!(queue.ready_nodes(), vec!["d"]);
    }

    #[test]
    fn initial_skipped_node_also_cascades_skip() {
        // Skipped 同为终态：new() 映射了 "skipped"，级联集合就必须包含它，
        // 否则下游永久 Pending、has_unsettled 恒真，与注释声明的防御语义不符。
        let def = definition(vec![node("a", &[]), node("b", &["a"]), node("d", &[])]);
        let mut initial = HashMap::new();
        initial.insert("a".to_string(), "skipped".to_string());
        let mut queue = ReadyQueue::new(&def, &initial);
        let skipped = queue.cascade_initial_terminal();
        assert_eq!(skipped, vec!["b"]);
        assert_eq!(queue.ready_nodes(), vec!["d"]);
        // 独立分支跑完后可正常收尾。
        queue.claim("d");
        queue.on_finished("d", FinishKind::Succeeded);
        assert!(!queue.has_unsettled());
    }

    #[test]
    fn on_finished_does_not_overwrite_state_settled_elsewhere() {
        // cancel_remaining 已把在途节点标为 Cancelled 后，迟到的执行结果
        // 不能把终态覆盖回 Succeeded/Failed。
        let def = definition(vec![node("a", &[]), node("b", &["a"])]);
        let mut queue = ReadyQueue::new(&def, &HashMap::new());
        queue.claim("a");
        queue.cancel_remaining();
        assert!(queue.on_finished("a", FinishKind::Succeeded).is_empty());
        assert!(queue.failed_nodes().is_empty());
        assert!(queue.skipped_nodes().is_empty());
        assert!(!queue.has_unsettled(), "a 应保持 Cancelled，b 保持 Cancelled");
    }
}

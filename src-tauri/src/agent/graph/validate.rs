//! 图定义校验：DAG 判环、依赖存在、agent 可用性、节点数与输出键约束。
//!
//! 校验失败返回聚合了全部问题的中文错误文本（以「错误：」开头），
//! 供编排器模型自修复，也供 `graph_plan_update` 命令直接回传前端。

use std::collections::{HashMap, HashSet};

use super::types::{GraphDefinition, GraphNodeAgent};

pub(crate) const MAX_GRAPH_NODES: usize = 20;

/// 校验时的执行 agent 可用性快照（由调用方装配）。
pub(crate) struct GraphAgentAvailability {
    /// 当前会话已启用的子智能体 id 列表。
    pub enabled_sub_agent_ids: HashSet<String>,
    pub claude_available: bool,
    pub codex_available: bool,
}

/// 校验图定义；返回聚合全部问题的错误（`Err(String)`，以「错误：」开头）。
pub(crate) fn validate_graph(
    definition: &GraphDefinition,
    availability: &GraphAgentAvailability,
) -> Result<(), String> {
    let mut errors: Vec<String> = Vec::new();

    if definition.title.trim().is_empty() {
        errors.push("title 不能为空".to_string());
    }
    if definition.nodes.is_empty() {
        errors.push("nodes 不能为空：执行图至少需要一个节点".to_string());
    }
    if definition.nodes.len() > MAX_GRAPH_NODES {
        errors.push(format!(
            "节点数 {} 超过上限 {}",
            definition.nodes.len(),
            MAX_GRAPH_NODES
        ));
    }

    // ── 节点 id / outputKey 唯一性 ─────────────────────────────────────────
    let mut ids: HashSet<&str> = HashSet::new();
    let mut output_keys: HashSet<&str> = HashSet::new();
    for node in &definition.nodes {
        let id = node.id.trim();
        if id.is_empty() {
            errors.push("存在 id 为空的节点".to_string());
        } else if !ids.insert(id) {
            errors.push(format!("节点 id '{id}' 重复"));
        }

        let output_key = node.output_key.trim();
        if output_key.is_empty() {
            errors.push(format!("节点 '{id}' 的 outputKey 不能为空"));
        } else if !output_keys.insert(output_key) {
            errors.push(format!(
                "outputKey '{output_key}' 被多个节点使用（节点 '{id}'）"
            ));
        }
    }

    // ── 依赖存在 / 自依赖 / 重复依赖 ────────────────────────────────────────
    for node in &definition.nodes {
        let id = node.id.trim();
        let mut seen_deps: HashSet<&str> = HashSet::new();
        for dep in &node.depends_on {
            let dep = dep.trim();
            if dep == id {
                errors.push(format!("节点 '{id}' 不能依赖自身"));
            } else if !ids.contains(dep) {
                errors.push(format!("节点 '{id}' 依赖了不存在的节点 '{dep}'"));
            }
            if !seen_deps.insert(dep) {
                errors.push(format!("节点 '{id}' 的 dependsOn 中 '{dep}' 重复"));
            }
        }
    }

    // ── injectStateKeys ⊆ stateKeys ∪ 全部 outputKey ───────────────────────
    let declared_keys: HashSet<&str> = definition
        .state_keys
        .iter()
        .map(|entry| entry.key.trim())
        .chain(output_keys.iter().copied())
        .collect();
    for node in &definition.nodes {
        for key in &node.inject_state_keys {
            let key = key.trim();
            if !declared_keys.contains(key) {
                errors.push(format!(
                    "节点 '{}' 的 injectStateKeys 引用了未声明的 state key '{key}'（需在 stateKeys 中声明，或为某个节点的 outputKey）",
                    node.id.trim()
                ));
            }
        }
    }

    // ── agent 可用性 ───────────────────────────────────────────────────────
    for node in &definition.nodes {
        let id = node.id.trim();
        match &node.agent {
            GraphNodeAgent::SubAgent { agent_id } => {
                let agent_id = agent_id.trim();
                if agent_id.is_empty() {
                    errors.push(format!("节点 '{id}' 的 agentId 不能为空"));
                } else if !availability.enabled_sub_agent_ids.contains(agent_id) {
                    errors.push(format!(
                        "节点 '{id}' 引用的子智能体 '{agent_id}' 未启用或不存在（可用列表见系统提示）"
                    ));
                }
            }
            GraphNodeAgent::Claude => {
                if !availability.claude_available {
                    errors.push(format!(
                        "节点 '{id}' 使用 claude，但应用设置中未配置 claude 可执行文件路径"
                    ));
                }
            }
            GraphNodeAgent::Codex => {
                if !availability.codex_available {
                    errors.push(format!(
                        "节点 '{id}' 使用 codex，但应用设置中未配置 codex 可执行文件路径"
                    ));
                }
            }
        }
        if node.task.trim().is_empty() {
            errors.push(format!("节点 '{id}' 的 task 不能为空"));
        }
    }

    // ── DAG 判环（拓扑排序） ────────────────────────────────────────────────
    if errors.is_empty() {
        if let Err(cycle_error) = topological_layers(definition) {
            errors.push(cycle_error);
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "错误：图定义校验未通过：\n- {}",
            errors.join("\n- ")
        ))
    }
}

/// Kahn 拓扑分层：同层节点互相无依赖可并行，跨层严格串行。
/// 返回按执行顺序排列的节点 id 分层列表；存在环时报错。
pub(crate) fn topological_layers(definition: &GraphDefinition) -> Result<Vec<Vec<String>>, String> {
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();

    for node in &definition.nodes {
        in_degree.entry(node.id.as_str()).or_insert(0);
    }
    for node in &definition.nodes {
        for dep in &node.depends_on {
            *in_degree.entry(node.id.as_str()).or_insert(0) += 1;
            dependents
                .entry(dep.as_str())
                .or_default()
                .push(node.id.as_str());
        }
    }

    let mut layers: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<&str> = in_degree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| *id)
        .collect();
    current.sort_unstable();

    let mut placed = 0usize;
    while !current.is_empty() {
        placed += current.len();
        layers.push(current.iter().map(|id| id.to_string()).collect());

        let mut next: Vec<&str> = Vec::new();
        for id in &current {
            if let Some(children) = dependents.get(id) {
                for child in children {
                    let degree = in_degree
                        .get_mut(child)
                        .ok_or_else(|| format!("节点 '{child}' 依赖关系异常"))?;
                    *degree -= 1;
                    if *degree == 0 {
                        next.push(child);
                    }
                }
            }
        }
        next.sort_unstable();
        next.dedup();
        current = next;
    }

    if placed != definition.nodes.len() {
        return Err(format!(
            "节点依赖存在环（仅 {} / {} 个节点可拓扑排序），请调整 dependsOn 解除循环依赖",
            placed,
            definition.nodes.len()
        ));
    }

    Ok(layers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::graph::types::{GraphNode, GraphStateKey};

    fn availability() -> GraphAgentAvailability {
        GraphAgentAvailability {
            enabled_sub_agent_ids: HashSet::from(["browser-agent".to_string()]),
            claude_available: true,
            codex_available: true,
        }
    }

    fn node(id: &str, depends_on: &[&str], agent: GraphNodeAgent) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            title: format!("节点{id}"),
            role: String::new(),
            agent,
            task: "做点什么".to_string(),
            depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
            inject_state_keys: Vec::new(),
            output_key: format!("out_{id}"),
        }
    }

    fn definition(nodes: Vec<GraphNode>) -> GraphDefinition {
        GraphDefinition {
            title: "测试图".to_string(),
            summary: String::new(),
            state_keys: Vec::new(),
            nodes,
        }
    }

    #[test]
    fn valid_graph_passes() {
        let def = definition(vec![
            node("n1", &[], GraphNodeAgent::Claude),
            node("n2", &["n1"], GraphNodeAgent::Codex),
        ]);
        assert!(validate_graph(&def, &availability()).is_ok());
    }

    #[test]
    fn cycle_is_rejected() {
        let def = definition(vec![
            node("n1", &["n2"], GraphNodeAgent::Claude),
            node("n2", &["n1"], GraphNodeAgent::Codex),
        ]);
        let error = validate_graph(&def, &availability()).unwrap_err();
        assert!(error.contains("环"), "unexpected error: {error}");
    }

    #[test]
    fn missing_dependency_is_rejected() {
        let def = definition(vec![node("n1", &["ghost"], GraphNodeAgent::Claude)]);
        let error = validate_graph(&def, &availability()).unwrap_err();
        assert!(error.contains("不存在的节点"), "unexpected error: {error}");
    }

    #[test]
    fn duplicate_output_key_is_rejected() {
        let mut def = definition(vec![
            node("n1", &[], GraphNodeAgent::Claude),
            node("n2", &[], GraphNodeAgent::Codex),
        ]);
        def.nodes[1].output_key = "out_n1".to_string();
        let error = validate_graph(&def, &availability()).unwrap_err();
        assert!(error.contains("outputKey"), "unexpected error: {error}");
    }

    #[test]
    fn unavailable_agent_is_rejected() {
        let def = definition(vec![node(
            "n1",
            &[],
            GraphNodeAgent::SubAgent {
                agent_id: "missing-agent".to_string(),
            },
        )]);
        let error = validate_graph(&def, &availability()).unwrap_err();
        assert!(error.contains("未启用或不存在"), "unexpected error: {error}");
    }

    #[test]
    fn undeclared_inject_key_is_rejected() {
        let mut def = definition(vec![node("n1", &[], GraphNodeAgent::Claude)]);
        def.nodes[0].inject_state_keys = vec!["nope".to_string()];
        let error = validate_graph(&def, &availability()).unwrap_err();
        assert!(error.contains("未声明的 state key"), "unexpected error: {error}");
    }

    #[test]
    fn inject_key_can_reference_output_key() {
        let mut def = definition(vec![
            node("n1", &[], GraphNodeAgent::Claude),
            node("n2", &["n1"], GraphNodeAgent::Codex),
        ]);
        def.nodes[1].inject_state_keys = vec!["out_n1".to_string()];
        assert!(validate_graph(&def, &availability()).is_ok());
    }

    #[test]
    fn state_keys_declaration_allows_inject() {
        let mut def = definition(vec![node("n1", &[], GraphNodeAgent::Claude)]);
        def.state_keys = vec![GraphStateKey {
            key: "shared".to_string(),
            description: String::new(),
        }];
        def.nodes[0].inject_state_keys = vec!["shared".to_string()];
        assert!(validate_graph(&def, &availability()).is_ok());
    }

    #[test]
    fn too_many_nodes_is_rejected() {
        let nodes = (0..MAX_GRAPH_NODES + 1)
            .map(|index| node(&format!("n{index}"), &[], GraphNodeAgent::Claude))
            .collect();
        let def = definition(nodes);
        let error = validate_graph(&def, &availability()).unwrap_err();
        assert!(error.contains("超过上限"), "unexpected error: {error}");
    }

    #[test]
    fn topological_layers_groups_independent_nodes() {
        let def = definition(vec![
            node("n1", &[], GraphNodeAgent::Claude),
            node("n2", &[], GraphNodeAgent::Codex),
            node("n3", &["n1", "n2"], GraphNodeAgent::Claude),
        ]);
        let layers = topological_layers(&def).unwrap();
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0], vec!["n1".to_string(), "n2".to_string()]);
        assert_eq!(layers[1], vec!["n3".to_string()]);
    }
}

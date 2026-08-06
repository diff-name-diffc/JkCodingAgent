//! PI v3 图定义校验：结构、DAG、Harness 引用 + 语义规则（时序/冲突/验证节点）统一 fail-fast。
//!
//! 语义规则（v3 新增，关闭 v2 的静默失败面）：
//! - injectStateKeys 的生产者必须是消费者的严格拓扑祖先；无生产者的键必须来自
//!   seeded_keys（修复图继承的 state），否则运行期必然注入不到任何值。
//! - 可能并行的两个 coding 节点若 expectedFiles 相交则报错（写冲突预检）。
//! - 含 coding 节点的图必须至少有一个 read_only 节点依赖其产物（验证节点强制）。

use std::collections::{HashMap, HashSet};

use super::types::{
    BaseToolGroup, GraphDefinition, GraphHarnessCatalog, GRAPH_DEFINITION_VERSION,
};

pub(crate) const MAX_GRAPH_NODES: usize = 20;

/// `seeded_keys`：运行期 state 中已存在的键（修复图继承的 state；普通图为空）。
pub(crate) fn validate_graph(
    definition: &GraphDefinition,
    catalog: &GraphHarnessCatalog,
    seeded_keys: &HashSet<String>,
) -> Result<(), String> {
    let mut errors = Vec::new();
    if definition.version != GRAPH_DEFINITION_VERSION {
        errors.push(format!(
            "version 必须为 {GRAPH_DEFINITION_VERSION}，当前为 {}",
            definition.version
        ));
    }
    if definition.title.trim().is_empty() {
        errors.push("title 不能为空".to_string());
    }
    if definition.nodes.is_empty() {
        errors.push("nodes 不能为空：执行图至少需要一个节点".to_string());
    }
    if definition.nodes.len() > MAX_GRAPH_NODES {
        errors.push(format!(
            "节点数 {} 超过上限 {MAX_GRAPH_NODES}",
            definition.nodes.len()
        ));
    }

    let model_ids = catalog
        .models
        .iter()
        .map(|model| model.id.as_str())
        .collect::<HashSet<_>>();
    let tool_refs = catalog
        .tools
        .iter()
        .map(|tool| (tool.source.as_str(), tool.name.as_str()))
        .collect::<HashSet<_>>();
    let mut ids = HashSet::new();
    let mut output_keys = HashSet::new();
    for node in &definition.nodes {
        let id = node.id.trim();
        if id.is_empty() {
            errors.push("存在 id 为空的节点".to_string());
        } else if !ids.insert(id) {
            errors.push(format!("节点 id '{id}' 重复"));
        }
        if node.title.trim().is_empty() {
            errors.push(format!("节点 '{id}' 的 title 不能为空"));
        }
        if node.task.trim().is_empty() {
            errors.push(format!("节点 '{id}' 的 task 不能为空"));
        }
        if !model_ids.contains(node.model_ref.trim()) {
            errors.push(format!(
                "节点 '{id}' 引用了不存在或已禁用的模型 '{}'",
                node.model_ref
            ));
        }
        let output_key = node.output_key.trim();
        if output_key.is_empty() {
            errors.push(format!("节点 '{id}' 的 outputKey 不能为空"));
        } else if !output_keys.insert(output_key) {
            errors.push(format!("outputKey '{output_key}' 被多个节点使用"));
        }
        let mut selected = HashSet::new();
        for tool in &node.special_tools {
            if !matches!(tool.source.as_str(), "pi_extension" | "aha") {
                errors.push(format!(
                    "节点 '{id}' 的工具 '{}' 来源 '{}' 非法",
                    tool.name, tool.source
                ));
            } else if !tool_refs.contains(&(tool.source.as_str(), tool.name.as_str())) {
                errors.push(format!(
                    "节点 '{id}' 引用了不可用工具 '{}:{}'",
                    tool.source, tool.name
                ));
            }
            if !selected.insert((tool.source.as_str(), tool.name.as_str())) {
                errors.push(format!(
                    "节点 '{id}' 重复选择工具 '{}:{}'",
                    tool.source, tool.name
                ));
            }
        }
    }

    for node in &definition.nodes {
        let id = node.id.trim();
        let mut seen = HashSet::new();
        for dep in &node.depends_on {
            let dep = dep.trim();
            if dep == id {
                errors.push(format!("节点 '{id}' 不能依赖自身"));
            } else if !ids.contains(dep) {
                errors.push(format!("节点 '{id}' 依赖了不存在的节点 '{dep}'"));
            }
            if !seen.insert(dep) {
                errors.push(format!("节点 '{id}' 的 dependsOn 中 '{dep}' 重复"));
            }
        }
    }

    // 环检测独立于其他结构错误：它是语义规则的唯一前提。
    let mut acyclic = true;
    if let Err(error) = topological_layers(definition) {
        errors.push(error);
        acyclic = false;
    }

    // 语义规则只依赖无环前提，不因版本/模型引用等其他错误而跳过：
    // 一次性返回全部问题，避免用户「修一个错→再撞下一批错」的多轮往返。
    if acyclic {
        validate_semantics(definition, seeded_keys, &mut errors);
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

fn validate_semantics(
    definition: &GraphDefinition,
    seeded_keys: &HashSet<String>,
    errors: &mut Vec<String>,
) {
    let ancestors = transitive_ancestors(definition);
    let producers: HashMap<&str, &str> = definition
        .nodes
        .iter()
        .map(|node| (node.output_key.trim(), node.id.trim()))
        .collect();

    // 1) injectStateKeys 时序：生产者必须是严格祖先；无生产者则必须已种入 state。
    for node in &definition.nodes {
        let id = node.id.trim();
        let node_ancestors = ancestors.get(id);
        for key in &node.inject_state_keys {
            let key = key.trim();
            if key.is_empty() {
                continue;
            }
            match producers.get(key) {
                Some(producer) => {
                    let is_ancestor =
                        node_ancestors.is_some_and(|set| set.contains(*producer));
                    if !is_ancestor {
                        errors.push(format!(
                            "节点 '{id}' 注入的 state key '{key}' 由节点 '{producer}' 产出，但 '{producer}' 不是它的上游依赖：运行到该节点时 state 中还没有这个值，请补充 dependsOn"
                        ));
                    }
                }
                None => {
                    if !seeded_keys.contains(key) {
                        errors.push(format!(
                            "节点 '{id}' 注入的 state key '{key}' 没有任何节点产出，也不在继承的共享 state 中：运行期将注入不到任何值"
                        ));
                    }
                }
            }
        }
    }

    // 2) 并行写冲突预检：互不为祖先的两个 coding 节点 expectedFiles 相交 → 报错。
    let coding_nodes = definition
        .nodes
        .iter()
        .filter(|node| node.base_tool_group == BaseToolGroup::Coding && !node.expected_files.is_empty())
        .collect::<Vec<_>>();
    for (i, left) in coding_nodes.iter().enumerate() {
        for right in coding_nodes.iter().skip(i + 1) {
            let left_id = left.id.trim();
            let right_id = right.id.trim();
            let related = ancestors
                .get(left_id)
                .is_some_and(|set| set.contains(right_id))
                || ancestors
                    .get(right_id)
                    .is_some_and(|set| set.contains(left_id));
            if related {
                continue;
            }
            let left_files = left
                .expected_files
                .iter()
                .map(|file| file.trim())
                .collect::<HashSet<_>>();
            let conflict = right
                .expected_files
                .iter()
                .map(|file| file.trim())
                .any(|file| left_files.contains(file));
            if conflict {
                errors.push(format!(
                    "节点 '{left_id}' 与 '{right_id}' 可能并行执行且 expectedFiles 相交：两个写节点同时改同一文件会互相覆盖，请通过 dependsOn 串行化或拆分文件范围"
                ));
            }
        }
    }

    // 3) 验证节点强制：含 coding 节点的图必须有一个 read_only 节点以其为上游。
    let has_coding = definition
        .nodes
        .iter()
        .any(|node| node.base_tool_group == BaseToolGroup::Coding);
    if has_coding {
        let verified = definition.nodes.iter().any(|node| {
            if node.base_tool_group != BaseToolGroup::ReadOnly {
                return false;
            }
            ancestors.get(node.id.trim()).is_some_and(|set| {
                definition.nodes.iter().any(|upstream| {
                    upstream.base_tool_group == BaseToolGroup::Coding
                        && set.contains(upstream.id.trim())
                })
            })
        });
        if !verified {
            errors.push(
                "包含修改类（coding）节点的执行图必须至少有一个只读验证节点依赖其产出（例如读取改动文件、运行测试并核对结果）：请补充验证节点"
                    .to_string(),
            );
        }
    }
}

/// 每个节点的严格传递祖先集（id 已 trim）。
pub(crate) fn transitive_ancestors(definition: &GraphDefinition) -> HashMap<String, HashSet<String>> {
    let mut direct: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut ids = Vec::new();
    for node in &definition.nodes {
        let id = node.id.trim();
        ids.push(id);
        direct.insert(
            id,
            node.depends_on.iter().map(|dep| dep.trim()).collect(),
        );
    }
    // 拓扑序（Kahn）上逐层累积祖先集，保证计算祖先时其依赖已算完。
    let layers = topological_layers(definition).unwrap_or_default();
    let mut result: HashMap<String, HashSet<String>> = HashMap::new();
    for layer in layers {
        for id in layer {
            let mut set = HashSet::new();
            for dep in direct.get(id.as_str()).into_iter().flatten() {
                set.insert(dep.to_string());
                if let Some(dep_ancestors) = result.get(*dep) {
                    set.extend(dep_ancestors.iter().cloned());
                }
            }
            result.insert(id, set);
        }
    }
    // 环等异常场景兜底：未入层的节点给空集。
    for id in ids {
        result.entry(id.to_string()).or_default();
    }
    result
}

pub(crate) fn topological_layers(definition: &GraphDefinition) -> Result<Vec<Vec<String>>, String> {
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    for node in &definition.nodes {
        in_degree.entry(node.id.trim()).or_insert(0);
    }
    for node in &definition.nodes {
        for dep in &node.depends_on {
            *in_degree.entry(node.id.trim()).or_insert(0) += 1;
            dependents
                .entry(dep.trim())
                .or_default()
                .push(node.id.trim());
        }
    }
    let mut current = in_degree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();
    current.sort_unstable();
    let mut layers = Vec::new();
    let mut placed = 0;
    while !current.is_empty() {
        placed += current.len();
        layers.push(current.iter().map(|id| id.to_string()).collect());
        let mut next = Vec::new();
        for id in &current {
            if let Some(children) = dependents.get(id) {
                for child in children {
                    let degree = in_degree
                        .get_mut(child)
                        .ok_or_else(|| format!("节点 '{child}' 依赖关系异常"))?;
                    *degree -= 1;
                    if *degree == 0 {
                        next.push(*child);
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
            "节点依赖存在环（仅 {placed} / {} 个节点可拓扑排序）",
            definition.nodes.len()
        ));
    }
    Ok(layers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::graph::types::{GraphNode, GraphStateKey};

    fn catalog() -> GraphHarnessCatalog {
        GraphHarnessCatalog {
            models: vec![crate::agent::graph::types::GraphHarnessModel {
                id: "m1".into(),
                label: "M1".into(),
                model: "m1".into(),
                category: "text".into(),
                capabilities: vec![],
            }],
            tools: vec![],
            diagnostics: vec![],
        }
    }
    fn node(id: &str, deps: &[&str]) -> GraphNode {
        GraphNode {
            id: id.into(),
            title: id.into(),
            role: String::new(),
            model_ref: "m1".into(),
            base_tool_group: BaseToolGroup::ReadOnly,
            special_tools: vec![],
            task: "task".into(),
            depends_on: deps.iter().map(|v| v.to_string()).collect(),
            inject_state_keys: vec![],
            output_key: format!("out_{id}"),
            expected_files: vec![],
            export_policy: Default::default(),
        }
    }
    fn coding(id: &str, deps: &[&str]) -> GraphNode {
        let mut n = node(id, deps);
        n.base_tool_group = BaseToolGroup::Coding;
        n
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
    fn seeded() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn validates_v3_graph_with_verification_node() {
        assert!(validate_graph(
            &definition(vec![node("a", &[]), coding("b", &["a"]), node("c", &["b"])]),
            &catalog(),
            &seeded()
        )
        .is_ok());
    }

    #[test]
    fn rejects_v2_version() {
        let mut def = definition(vec![node("a", &[])]);
        def.version = 2;
        assert!(validate_graph(&def, &catalog(), &seeded())
            .unwrap_err()
            .contains("version"));
    }

    #[test]
    fn rejects_cycle() {
        assert!(validate_graph(
            &definition(vec![node("a", &["b"]), node("b", &["a"])]),
            &catalog(),
            &seeded()
        )
        .unwrap_err()
        .contains("环"));
    }

    #[test]
    fn rejects_missing_model() {
        let mut def = definition(vec![node("a", &[])]);
        def.nodes[0].model_ref = "missing".into();
        assert!(validate_graph(&def, &catalog(), &seeded())
            .unwrap_err()
            .contains("模型"));
    }

    #[test]
    fn trims_ids_consistently_when_sorting() {
        let definition = definition(vec![node(" a ", &[]), node("b", &[" a "])]);
        assert!(validate_graph(&definition, &catalog(), &seeded()).is_ok());
        assert_eq!(
            topological_layers(&definition).unwrap(),
            vec![vec!["a".to_string()], vec!["b".to_string()]]
        );
    }

    #[test]
    fn rejects_inject_key_produced_by_non_ancestor() {
        // c 注入 out_b，但 b 与 c 并行（b 不是 c 的祖先）。
        let mut def = definition(vec![node("a", &[]), coding("b", &["a"]), node("c", &["a"])]);
        def.nodes[2].inject_state_keys = vec!["out_b".into()];
        def.nodes.push(node("v", &["b", "c"]));
        let error = validate_graph(&def, &catalog(), &seeded()).unwrap_err();
        assert!(error.contains("out_b"));
        assert!(error.contains("dependsOn"));
    }

    #[test]
    fn allows_inject_key_from_ancestor() {
        let mut def = definition(vec![node("a", &[]), coding("b", &["a"]), node("c", &["b"])]);
        def.nodes[2].inject_state_keys = vec!["out_b".into()];
        assert!(validate_graph(&def, &catalog(), &seeded()).is_ok());
    }

    #[test]
    fn rejects_inject_key_without_producer_or_seed() {
        let mut def = definition(vec![node("a", &[])]);
        def.nodes[0].inject_state_keys = vec!["ghost".into()];
        let error = validate_graph(&def, &catalog(), &seeded()).unwrap_err();
        assert!(error.contains("ghost"));
    }

    #[test]
    fn allows_inject_key_seeded_from_inherited_state() {
        let mut def = definition(vec![node("a", &[])]);
        def.nodes[0].inject_state_keys = vec!["auth_analysis".into()];
        def.state_keys = vec![GraphStateKey {
            key: "auth_analysis".into(),
            description: "继承结论".into(),
        }];
        let seeded = HashSet::from(["auth_analysis".to_string()]);
        assert!(validate_graph(&def, &catalog(), &seeded).is_ok());
    }

    #[test]
    fn rejects_parallel_coding_nodes_with_conflicting_files() {
        let mut left = coding("l", &[]);
        left.expected_files = vec!["src/main.rs".into()];
        let mut right = coding("r", &[]);
        right.expected_files = vec!["src/main.rs".into()];
        let mut def = definition(vec![left, right, node("v", &["l", "r"])]);
        def.nodes[2].depends_on = vec!["l".into(), "r".into()];
        let error = validate_graph(&def, &catalog(), &seeded()).unwrap_err();
        assert!(error.contains("src/main.rs") || error.contains("并行"));
    }

    #[test]
    fn allows_serialized_coding_nodes_with_same_files() {
        let mut left = coding("l", &[]);
        left.expected_files = vec!["src/main.rs".into()];
        let mut right = coding("r", &["l"]);
        right.expected_files = vec!["src/main.rs".into()];
        let def = definition(vec![left, right, node("v", &["r"])]);
        assert!(validate_graph(&def, &catalog(), &seeded()).is_ok());
    }

    #[test]
    fn requires_verification_node_for_coding_graph() {
        let error = validate_graph(
            &definition(vec![node("a", &[]), coding("b", &["a"])]),
            &catalog(),
            &seeded(),
        )
        .unwrap_err();
        assert!(error.contains("验证节点"));
    }

    #[test]
    fn pure_readonly_graph_needs_no_verification_node() {
        assert!(
            validate_graph(&definition(vec![node("a", &[]), node("b", &["a"])]), &catalog(), &seeded())
                .is_ok()
        );
    }
}

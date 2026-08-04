//! PI v2 图定义校验：结构、DAG、模型和 Harness 工具引用统一 fail-fast。

use std::collections::{HashMap, HashSet};

use super::types::{GraphDefinition, GraphHarnessCatalog};

pub(crate) const MAX_GRAPH_NODES: usize = 20;

pub(crate) fn validate_graph(
    definition: &GraphDefinition,
    catalog: &GraphHarnessCatalog,
) -> Result<(), String> {
    let mut errors = Vec::new();
    if definition.version != 2 {
        errors.push(format!("version 必须为 2，当前为 {}", definition.version));
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

    let declared_keys = definition
        .state_keys
        .iter()
        .map(|entry| entry.key.trim())
        .chain(output_keys.iter().copied())
        .collect::<HashSet<_>>();
    for node in &definition.nodes {
        for key in &node.inject_state_keys {
            if !declared_keys.contains(key.trim()) {
                errors.push(format!(
                    "节点 '{}' 的 injectStateKeys 引用了未声明的 key '{}'",
                    node.id, key
                ));
            }
        }
    }
    if errors.is_empty() {
        if let Err(error) = topological_layers(definition) {
            errors.push(error);
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
    use crate::agent::graph::types::{BaseToolGroup, GraphHarnessModel, GraphNode};

    fn catalog() -> GraphHarnessCatalog {
        GraphHarnessCatalog {
            models: vec![GraphHarnessModel {
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
            base_tool_group: BaseToolGroup::Coding,
            special_tools: vec![],
            task: "task".into(),
            depends_on: deps.iter().map(|v| v.to_string()).collect(),
            inject_state_keys: vec![],
            output_key: format!("out_{id}"),
        }
    }
    fn definition(nodes: Vec<GraphNode>) -> GraphDefinition {
        GraphDefinition {
            version: 2,
            title: "test".into(),
            summary: String::new(),
            state_keys: vec![],
            nodes,
        }
    }

    #[test]
    fn validates_v2_graph() {
        assert!(validate_graph(
            &definition(vec![node("a", &[]), node("b", &["a"])]),
            &catalog()
        )
        .is_ok());
    }
    #[test]
    fn rejects_cycle() {
        assert!(validate_graph(
            &definition(vec![node("a", &["b"]), node("b", &["a"])]),
            &catalog()
        )
        .unwrap_err()
        .contains("环"));
    }
    #[test]
    fn rejects_missing_model() {
        let mut def = definition(vec![node("a", &[])]);
        def.nodes[0].model_ref = "missing".into();
        assert!(validate_graph(&def, &catalog())
            .unwrap_err()
            .contains("模型"));
    }

    #[test]
    fn trims_ids_consistently_when_sorting() {
        let definition = definition(vec![node(" a ", &[]), node("b", &[" a "])]);
        assert!(validate_graph(&definition, &catalog()).is_ok());
        assert_eq!(
            topological_layers(&definition).unwrap(),
            vec![vec!["a".to_string()], vec!["b".to_string()]]
        );
    }
}

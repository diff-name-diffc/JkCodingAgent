//! 节点输入装配（runner 在层首为每个可执行节点调用一次）。

use std::collections::HashMap;

use serde_json::{Map, Value};

use super::types::GraphNode;

/// 节点输入 = 总体需求 + 角色 + 子任务 + 上游输出（dependsOn 逐个）+
/// 共享状态（仅 injectStateKeys 声明的 key）。
pub(super) fn assemble_node_input(
    user_requirement: &str,
    node: &GraphNode,
    outputs: &HashMap<String, String>,
    state: &Map<String, Value>,
) -> String {
    let mut sections = vec![format!("# 总体需求\n{}", user_requirement.trim())];

    if !node.role.trim().is_empty() {
        sections.push(format!("# 你的角色\n{}", node.role.trim()));
    }
    sections.push(format!("# 你的子任务\n{}", node.task.trim()));

    if !node.depends_on.is_empty() {
        let mut upstream = String::from("# 上游节点输出");
        for dep in &node.depends_on {
            if let Some(output) = outputs.get(dep) {
                upstream.push_str(&format!("\n\n## 节点 {dep} 的输出\n{output}"));
            }
        }
        sections.push(upstream);
    }

    if !node.inject_state_keys.is_empty() {
        let mut injected = Map::new();
        for key in &node.inject_state_keys {
            if let Some(value) = state.get(key) {
                injected.insert(key.clone(), value.clone());
            }
        }
        if !injected.is_empty() {
            let rendered = serde_json::to_string_pretty(&Value::Object(injected))
                .unwrap_or_else(|_| "{}".to_string());
            sections.push(format!("# 共享状态\n{rendered}"));
        }
    }

    sections.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::graph::types::GraphNodeAgent;

    fn node() -> GraphNode {
        GraphNode {
            id: "n2".to_string(),
            title: "改造后端".to_string(),
            role: "后端编码 Agent".to_string(),
            agent: GraphNodeAgent::Claude,
            task: "根据分析结论改造".to_string(),
            depends_on: vec!["n1".to_string()],
            inject_state_keys: vec!["auth_analysis".to_string(), "missing".to_string()],
            output_key: "backend_changes".to_string(),
        }
    }

    #[test]
    fn input_contains_requirement_role_task_upstream_and_injected_state() {
        let mut outputs = HashMap::new();
        outputs.insert("n1".to_string(), "分析结论全文".to_string());
        let mut state = Map::new();
        state.insert(
            "auth_analysis".to_string(),
            Value::String("共享结论".to_string()),
        );

        let input = assemble_node_input("原始需求", &node(), &outputs, &state);

        assert!(input.contains("# 总体需求\n原始需求"));
        assert!(input.contains("# 你的角色\n后端编码 Agent"));
        assert!(input.contains("# 你的子任务\n根据分析结论改造"));
        assert!(input.contains("## 节点 n1 的输出\n分析结论全文"));
        assert!(input.contains("\"auth_analysis\": \"共享结论\""));
        // 未声明/不存在的 key 不注入
        assert!(!input.contains("missing"));
    }

    #[test]
    fn input_omits_optional_sections_when_empty() {
        let mut minimal = node();
        minimal.role = String::new();
        minimal.depends_on = Vec::new();
        minimal.inject_state_keys = Vec::new();

        let input = assemble_node_input("原始需求", &minimal, &HashMap::new(), &Map::new());

        assert!(!input.contains("# 你的角色"));
        assert!(!input.contains("# 上游节点输出"));
        assert!(!input.contains("# 共享状态"));
    }
}

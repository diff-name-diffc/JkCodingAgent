//! PI v3 图定义校验：结构、DAG、Harness 引用 + 语义规则（时序/冲突/验证节点）统一 fail-fast。
//!
//! 语义规则（v3 新增，关闭 v2 的静默失败面）：
//! - injectStateKeys 的生产者必须是消费者的严格拓扑祖先；无生产者的键必须来自
//!   seeded_keys（修复图继承的 state），否则运行期必然注入不到任何值。
//! - 可能并行的两个 coding 节点若 expectedFiles 相交则报错（写冲突预检）。
//! - 含 coding 节点的图必须至少有一个 read_only 节点依赖其产物（验证节点强制）。

use std::collections::{HashMap, HashSet};

use super::types::{BaseToolGroup, GraphDefinition, GraphHarnessCatalog, GRAPH_DEFINITION_VERSION};

pub(crate) const MAX_GRAPH_NODES: usize = 20;
const MAX_GRAPH_DEFINITION_BYTES: usize = 256 * 1024;
const MAX_GRAPH_TITLE_CHARS: usize = 200;
const MAX_GRAPH_SUMMARY_CHARS: usize = 2_000;
const MAX_STATE_KEYS: usize = 64;
const MAX_NODE_ROLE_CHARS: usize = 1_000;
const MAX_NODE_TASK_CHARS: usize = 32_000;
const MAX_SPECIAL_TOOLS: usize = 16;
const MAX_DEPENDENCIES: usize = 20;
const MAX_INJECT_STATE_KEYS: usize = 64;
const MAX_EXPECTED_FILES: usize = 256;
const MAX_EXPECTED_PATH_CHARS: usize = 4_096;

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
    if definition.title.chars().count() > MAX_GRAPH_TITLE_CHARS {
        errors.push(format!("title 超过 {MAX_GRAPH_TITLE_CHARS} 字符上限"));
    }
    if definition.summary.chars().count() > MAX_GRAPH_SUMMARY_CHARS {
        errors.push(format!("summary 超过 {MAX_GRAPH_SUMMARY_CHARS} 字符上限"));
    }
    if definition.state_keys.len() > MAX_STATE_KEYS {
        errors.push(format!(
            "stateKeys 数量 {} 超过上限 {MAX_STATE_KEYS}",
            definition.state_keys.len()
        ));
    }
    if serde_json::to_vec(definition)
        .map(|bytes| bytes.len() > MAX_GRAPH_DEFINITION_BYTES)
        .unwrap_or(true)
    {
        errors.push(format!(
            "图定义序列化体积超过 {} KiB 上限",
            MAX_GRAPH_DEFINITION_BYTES / 1024
        ));
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
    let mut declared_state_keys = HashSet::new();
    for state_key in &definition.state_keys {
        let key = state_key.key.trim();
        if !valid_graph_identifier(key) {
            errors.push(format!(
                "stateKeys 中的 key '{key}' 非法：必须匹配 [A-Za-z][A-Za-z0-9_-]{{0,63}}"
            ));
        } else if !declared_state_keys.insert(key) {
            errors.push(format!("stateKeys 中的 key '{key}' 重复"));
        }
        if state_key.description.chars().count() > MAX_NODE_ROLE_CHARS {
            errors.push(format!(
                "state key '{key}' 的 description 超过 1000 字符上限"
            ));
        }
    }

    let mut ids = HashSet::new();
    let mut output_keys = HashSet::new();
    for node in &definition.nodes {
        let id = node.id.trim();
        if !valid_graph_identifier(id) {
            errors.push(format!(
                "节点 id '{id}' 非法：必须匹配 [A-Za-z][A-Za-z0-9_-]{{0,63}}"
            ));
        } else if !ids.insert(id) {
            errors.push(format!("节点 id '{id}' 重复"));
        }
        if node.title.trim().is_empty() {
            errors.push(format!("节点 '{id}' 的 title 不能为空"));
        }
        if node.title.chars().count() > MAX_GRAPH_TITLE_CHARS {
            errors.push(format!("节点 '{id}' 的 title 超过 200 字符上限"));
        }
        if node.role.chars().count() > MAX_NODE_ROLE_CHARS {
            errors.push(format!("节点 '{id}' 的 role 超过 1000 字符上限"));
        }
        if node.task.trim().is_empty() {
            errors.push(format!("节点 '{id}' 的 task 不能为空"));
        }
        if node.task.chars().count() > MAX_NODE_TASK_CHARS {
            errors.push(format!("节点 '{id}' 的 task 超过 32000 字符上限"));
        }
        if node.model_ref.trim().is_empty() || node.model_ref.chars().count() > 256 {
            errors.push(format!("节点 '{id}' 的 modelRef 必须为 1–256 字符"));
        }
        if !model_ids.contains(node.model_ref.trim()) {
            errors.push(format!(
                "节点 '{id}' 引用了不存在或已禁用的模型 '{}'",
                node.model_ref
            ));
        }
        let output_key = node.output_key.trim();
        if !valid_graph_identifier(output_key) {
            errors.push(format!(
                "节点 '{id}' 的 outputKey '{output_key}' 非法：必须匹配 [A-Za-z][A-Za-z0-9_-]{{0,63}}"
            ));
        } else if !output_keys.insert(output_key) {
            errors.push(format!("outputKey '{output_key}' 被多个节点使用"));
        }
        if node.special_tools.len() > MAX_SPECIAL_TOOLS {
            errors.push(format!(
                "节点 '{id}' 的 specialTools 数量超过上限 {MAX_SPECIAL_TOOLS}"
            ));
        }
        if node.depends_on.len() > MAX_DEPENDENCIES {
            errors.push(format!(
                "节点 '{id}' 的 dependsOn 数量超过上限 {MAX_DEPENDENCIES}"
            ));
        }
        if node.inject_state_keys.len() > MAX_INJECT_STATE_KEYS {
            errors.push(format!(
                "节点 '{id}' 的 injectStateKeys 数量超过上限 {MAX_INJECT_STATE_KEYS}"
            ));
        }
        if node.expected_files.len() > MAX_EXPECTED_FILES {
            errors.push(format!(
                "节点 '{id}' 的 expectedFiles 数量超过上限 {MAX_EXPECTED_FILES}"
            ));
        }
        for path in &node.expected_files {
            if !valid_expected_path(path) {
                errors.push(format!(
                    "节点 '{id}' 的 expectedFiles 路径 '{path}' 非法：必须是工作区内相对路径，不能包含空值、NUL、绝对路径或 '..' 段，且最长 {MAX_EXPECTED_PATH_CHARS} 字符"
                ));
            }
        }
        let mut selected = HashSet::new();
        for tool in &node.special_tools {
            if tool.source == "pi_extension" {
                errors.push(format!(
                    "节点 '{id}' 引用了已禁用的 PI 扩展工具 '{}'；请迁移为经 CapabilityBroker 托管的 Aha 工具",
                    tool.name
                ));
            } else if tool.source != "aha" {
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

fn valid_graph_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && value.len() <= 64
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
}

fn valid_expected_path(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > MAX_EXPECTED_PATH_CHARS
        || value.contains('\0')
        || value.starts_with('/')
        || value.starts_with('\\')
        || value.as_bytes().get(1) == Some(&b':')
    {
        return false;
    }
    !value.split(['/', '\\']).any(|component| component == "..")
}

fn validate_semantics(
    definition: &GraphDefinition,
    seeded_keys: &HashSet<String>,
    errors: &mut Vec<String>,
) {
    let ancestors = transitive_ancestors(definition);
    // outputKey → 生产者。重复 outputKey 已作为结构错误上报；这里记录歧义键，
    // 规则 1 对其跳过祖先判定——否则基于 HashMap 任意选中的生产者会给出
    // 误导性的「通过」或「非上游」结论。
    let mut producers: HashMap<&str, &str> = HashMap::new();
    let mut ambiguous_keys: HashSet<&str> = HashSet::new();
    for node in &definition.nodes {
        let key = node.output_key.trim();
        if producers.insert(key, node.id.trim()).is_some() {
            ambiguous_keys.insert(key);
        }
    }

    // 1) injectStateKeys 时序：生产者必须是严格祖先；无生产者则必须已种入 state。
    for node in &definition.nodes {
        let id = node.id.trim();
        let node_ancestors = ancestors.get(id);
        for key in &node.inject_state_keys {
            let key = key.trim();
            if key.is_empty() {
                // 与 id/title/task/outputKey 的空值检查同口径 fail-fast：
                // 静默跳过会让配置笔误拖到运行期注入不到值才暴露。
                errors.push(format!("节点 '{id}' 的 injectStateKeys 中存在空的 key"));
                continue;
            }
            if ambiguous_keys.contains(key) {
                continue;
            }
            match producers.get(key) {
                Some(producer) => {
                    let is_ancestor = node_ancestors.is_some_and(|set| set.contains(*producer));
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
        .filter(|node| {
            node.base_tool_group == BaseToolGroup::Coding && !node.expected_files.is_empty()
        })
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
            // 与运行期 resolve_path 对齐的路径归一化（./ 前缀、.. 折叠、
            // 分隔符统一），避免同一文件的不同写法漏判冲突，两个并行写
            // 节点在运行期改同一文件互相覆盖。
            let left_files: HashSet<String> = left
                .expected_files
                .iter()
                .map(|file| normalize_expected_path(file))
                .filter(|file| !file.is_empty())
                .collect();
            let conflict = right
                .expected_files
                .iter()
                .map(|file| normalize_expected_path(file))
                .filter(|file| !file.is_empty())
                .any(|file| left_files.contains(&file));
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

/// expectedFiles 冲突预检的路径归一化：统一 `/` 与 `\` 分隔符，去掉 `./`
/// 空段，折叠 `..`（与运行期 resolve_path 的语义对齐），使
/// './src/main.rs'、'src/a/../main.rs'、'src\\main.rs' 与 'src/main.rs'
/// 判定为同一文件。仅用于写冲突比较，不落库、不改原始定义。
fn normalize_expected_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.trim().split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}

/// 每个节点的严格传递祖先集（id 已 trim）。
pub(crate) fn transitive_ancestors(
    definition: &GraphDefinition,
) -> HashMap<String, HashSet<String>> {
    let mut direct: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut ids = Vec::new();
    for node in &definition.nodes {
        let id = node.id.trim();
        ids.push(id);
        direct.insert(id, node.depends_on.iter().map(|dep| dep.trim()).collect());
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
    // 只认实际存在的节点 id：缺失依赖已作为结构错误单独上报，拓扑排序忽略
    // 指向幻影节点的边——否则幻影节点永不入队，placed 计数失真，缺失依赖
    // 会被误报为「环」并连带跳过语义校验。同理，trim 后重复的节点 id 在
    // in_degree 中天然折叠为一个条目，收尾比较以去重后的条目数为准（重复
    // id 也已作为结构错误上报），避免假环误报。
    for node in &definition.nodes {
        for dep in &node.depends_on {
            let dep = dep.trim();
            if !in_degree.contains_key(dep) {
                continue;
            }
            *in_degree.entry(node.id.trim()).or_insert(0) += 1;
            dependents.entry(dep).or_default().push(node.id.trim());
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
    if placed != in_degree.len() {
        return Err(format!(
            "节点依赖存在环（仅 {placed} / {} 个节点可拓扑排序）",
            in_degree.len()
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
            output_key: format!("out_{}", id.trim()),
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
        assert!(validate_graph(
            &definition(vec![node("a", &[]), node("b", &["a"])]),
            &catalog(),
            &seeded()
        )
        .is_ok());
    }

    #[test]
    fn missing_dependency_is_not_misreported_as_cycle() {
        // 缺失依赖已单独报结构错误，不得再误判为「环」而跳过语义校验：
        // 两类问题应一次性返回，避免用户修完一轮再撞下一轮。
        let def = definition(vec![node("a", &["ghost"]), coding("b", &["a"])]);
        let error = validate_graph(&def, &catalog(), &seeded()).unwrap_err();
        assert!(error.contains("依赖了不存在的节点 'ghost'"));
        assert!(!error.contains("环"), "缺失依赖不应触发假环误报");
        assert!(error.contains("验证节点"), "无环前提下语义校验必须照常执行");
    }

    #[test]
    fn duplicate_ids_after_trim_are_not_misreported_as_cycle() {
        // trim 后重复的 id 在拓扑表中折叠为一个条目，不应造成假环；
        // 重复本身仍作为结构错误上报。
        let def = definition(vec![node("a", &[]), node(" a ", &[])]);
        let error = validate_graph(&def, &catalog(), &seeded()).unwrap_err();
        assert!(error.contains("重复"));
        assert!(!error.contains("环"));
    }

    #[test]
    fn real_cycle_is_still_reported() {
        // 回归：跳过缺失依赖边后，真实环的判定不受影响。
        let def = definition(vec![
            node("a", &["b"]),
            node("b", &["a"]),
            node("c", &["ghost"]),
        ]);
        let error = validate_graph(&def, &catalog(), &seeded()).unwrap_err();
        assert!(error.contains("环"));
        assert!(error.contains("依赖了不存在的节点 'ghost'"));
    }

    #[test]
    fn ambiguous_duplicate_output_key_skips_ancestor_rule() {
        // 重复 outputKey 已作为结构错误上报；生产者不明确时规则 1 跳过判定，
        // 不基于任意选中的生产者给出误导性「非上游」/「通过」结论。
        let mut def = definition(vec![node("a", &[]), coding("b", &["a"]), node("c", &["a"])]);
        def.nodes[0].output_key = "out_x".into();
        def.nodes[1].output_key = "out_x".into();
        def.nodes[2].inject_state_keys = vec!["out_x".into()];
        def.nodes.push(node("v", &["b", "c"]));
        let error = validate_graph(&def, &catalog(), &seeded()).unwrap_err();
        assert!(error.contains("outputKey 'out_x' 被多个节点使用"));
        assert!(
            !error.contains("不是它的上游依赖"),
            "歧义键不应触发规则 1 的误导性判定"
        );
    }

    #[test]
    fn rejects_empty_inject_state_key() {
        let mut def = definition(vec![node("a", &[])]);
        def.nodes[0].inject_state_keys = vec!["  ".into()];
        let error = validate_graph(&def, &catalog(), &seeded()).unwrap_err();
        assert!(error.contains("injectStateKeys 中存在空的 key"));
    }

    #[test]
    fn expected_files_conflict_uses_normalized_paths() {
        let variants = [
            ("./src/main.rs", "src/main.rs"),
            ("src/a/../main.rs", "src/main.rs"),
            ("src\\main.rs", "src/main.rs"),
        ];
        for (left_path, right_path) in variants {
            let mut left = coding("l", &[]);
            left.expected_files = vec![left_path.into()];
            let mut right = coding("r", &[]);
            right.expected_files = vec![right_path.into()];
            let def = definition(vec![left, right, node("v", &["l", "r"])]);
            let error = match validate_graph(&def, &catalog(), &seeded()) {
                Ok(()) => panic!("{left_path} 与 {right_path} 归一化后应判为冲突"),
                Err(error) => error,
            };
            assert!(error.contains("expectedFiles 相交"), "{error}");
        }
    }

    #[test]
    fn normalized_distinct_files_do_not_conflict() {
        let mut left = coding("l", &[]);
        left.expected_files = vec!["src/a.rs".into()];
        let mut right = coding("r", &[]);
        right.expected_files = vec!["./src/b.rs".into()];
        let def = definition(vec![left, right, node("v", &["l", "r"])]);
        assert!(validate_graph(&def, &catalog(), &seeded()).is_ok());
    }

    #[test]
    fn rejects_unbounded_graph_fields() {
        let mut def = definition(vec![node("a", &[])]);
        def.title = "x".repeat(MAX_GRAPH_TITLE_CHARS + 1);
        def.nodes[0].task = "x".repeat(MAX_NODE_TASK_CHARS + 1);

        let error = validate_graph(&def, &catalog(), &seeded()).unwrap_err();

        assert!(error.contains("title 超过"), "{error}");
        assert!(error.contains("task 超过"), "{error}");
    }

    #[test]
    fn rejects_invalid_identifiers_and_escaping_paths() {
        let mut def = definition(vec![node("1bad", &[])]);
        def.nodes[0].output_key = "bad key".into();
        def.nodes[0].expected_files = vec!["../outside.rs".into()];

        let error = validate_graph(&def, &catalog(), &seeded()).unwrap_err();

        assert!(error.contains("节点 id '1bad' 非法"), "{error}");
        assert!(error.contains("outputKey 'bad key' 非法"), "{error}");
        assert!(
            error.contains("expectedFiles 路径 '../outside.rs' 非法"),
            "{error}"
        );
    }
}

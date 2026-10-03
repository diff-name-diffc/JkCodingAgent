//! `graph_get` 与 draft 图节点级 CRUD 协议拦截：执行图感知与局部改写。
//!
//! 工具面按「单节点 CRUD、无冗余」设计：
//! - 读：`graph_get` 返回完整定义，单节点查询是其子集，不设独立工具；
//! - 改：`graph_node_update` 定点替换节点字段；
//! - 增：`graph_node_add` 追加节点，`insertBefore` 原子地把指定下游节点对本
//!   节点上游的依赖边改写到新节点（A→B 插成 A→N→B）；
//! - 删：`graph_node_delete` 默认拒绝删有下游依赖的节点（列出依赖者交回
//!   模型决策），`force=true` 时删下游传递闭包——injectStateKeys 的生产者
//!   必须是拓扑祖先，引用其产出的节点必在闭包内，级联删除即无悬空引用。
//!
//! 全部仅 draft 态可用；已运行图的修复走 submit_graph + inheritsFrom 新图
//! （历史 attempt 的节点快照不受定义改写影响）。

use std::collections::HashSet;

use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::agent::db::DispatcherDb;
use crate::agent::graph::commands::apply_draft_definition_update;
use crate::agent::graph::types::{
    BaseToolGroup, ExportPolicy, GraphDefinition, GraphNode, GraphPlanRecord, PLAN_DRAFT,
};
use crate::agent::graph::GraphStore;

/// graph_get 输出中每个共享 state 值的截断上限（字符）：感知键名与大致内容
/// 即可，全文保留在节点运行记录与 plan.state_json 中。
const STATE_VALUE_PREVIEW_CHARS: usize = 500;

/// 节点级变更拦截（add / update / delete）的统一结果：成功为确认文本
/// （不收口本轮），失败为可重试错误交回模型自修复。
pub(crate) enum GraphNodeMutationOutcome {
    Applied { text: String },
    Rejected { error: String },
}

/// 编排器侧图计划取数与口径（graph_get / graph_node_update 共用）：缺省取会话
/// 最近计划；显式 planId 校验存在与归属。错误即面向模型的反馈文本。
async fn resolve_plan(
    store: &GraphStore,
    workspace_id: &str,
    arguments: &Value,
) -> std::result::Result<GraphPlanRecord, String> {
    let plan_id_arg = arguments
        .get("planId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    let explicit_plan_id = plan_id_arg.is_some();
    let plan = match plan_id_arg {
        Some(plan_id) => store
            .get_plan_async(&plan_id)
            .await
            .map_err(|error| format!("错误：读取图计划失败：{error:#}"))?,
        None => store
            .latest_plan_for_workspace_async(workspace_id)
            .await
            .map_err(|error| format!("错误：读取会话最近图计划失败：{error:#}"))?,
    };
    let Some(plan) = plan else {
        // 「错误：」前缀仅用于显式 planId 拼错的场景（与 graph_plan_report 的
        // 口径一致，审查项 G8-17）；「从未出图」是说明性文本，不是错误。
        return Err(if explicit_plan_id {
            "错误：指定的 planId 不存在或已被清理；可不带 planId 取会话最近的图计划，或修正 planId 后重试。".to_string()
        } else {
            "当前会话还没有执行图。若任务复杂，请先探索项目后用 submit_graph 出图。".to_string()
        });
    };
    if plan.workspace_id != workspace_id {
        return Err("错误：指定的 planId 不属于当前会话。".to_string());
    }
    Ok(plan)
}

/// `graph_get` 协议拦截：返回图计划感知载荷（JSON 文本，不收口）。
/// definition 全文返回（task 是感知核心）；state 只给键 + 截断值防上下文膨胀。
pub(crate) async fn build_graph_get(
    db: &DispatcherDb,
    workspace_id: &str,
    arguments: &Value,
) -> Result<String> {
    let store = GraphStore::new(db);
    // 取数失败（无图/planId 拼错/跨会话）是面向模型的反馈而非内部错误：
    // 与 graph_plan_report 同口径，作为正常工具结果返回让模型自行决策。
    let plan = match resolve_plan(&store, workspace_id, arguments).await {
        Ok(plan) => plan,
        Err(feedback) => return Ok(feedback),
    };

    // definition 损坏属于存储级故障（正常写入路径都经校验）：显式失败优于
    // 返回半截感知载荷让模型基于错误信息决策。
    let definition: Value = serde_json::from_str(&plan.definition_json)
        .map_err(|error| anyhow::anyhow!("图定义损坏（JSON 解析失败：{error}）"))?;

    // 共享 state 键值预览；损坏时降级为告警字段（键信息缺失会误导
    // injectStateKeys，必须显式标出而不是静默省略）。
    let mut state_warning = Value::Null;
    let state_keys: Value = match serde_json::from_str::<serde_json::Map<String, Value>>(
        &plan.state_json,
    ) {
        Ok(state) if !state.is_empty() => {
            let truncated = state
                .iter()
                .map(|(key, value)| {
                    // 字符级截断（UTF-8 安全），并标注被截断的事实。
                    let raw = value.to_string();
                    let chars: Vec<char> = raw.chars().collect();
                    if chars.len() > STATE_VALUE_PREVIEW_CHARS {
                        json!({
                            "key": key,
                            "value": chars.into_iter().take(STATE_VALUE_PREVIEW_CHARS).collect::<String>(),
                            "truncated": true,
                        })
                    } else {
                        json!({ "key": key, "value": raw })
                    }
                })
                .collect::<Vec<_>>();
            Value::Array(truncated)
        }
        Ok(_) => json!([]),
        Err(error) => {
            state_warning = json!(format!("共享 state 解析失败（{error}），下列键信息不可用"));
            json!([])
        }
    };

    // 最近运行摘要：优先与 latest_run_id 一致的 run，找不到退回 runs.first()
    //（attempt_no DESC），与 graph_plan_report 的防御口径一致（G8-18）。
    let latest_run = plan
        .latest_run_id
        .as_deref()
        .and_then(|run_id| plan.runs.iter().find(|run| run.id == run_id))
        .or_else(|| plan.runs.first())
        .map(|run| {
            json!({
                "runId": run.id,
                "attemptNo": run.attempt_no,
                "status": run.status,
                "mode": run.mode,
                "verdictStatus": run.verdict_status,
                "verdictReason": run.verdict_reason,
            })
        })
        .unwrap_or(Value::Null);

    let payload = json!({
        "planId": plan.id,
        "title": plan.title,
        "status": plan.status,
        "requirement": plan.requirement,
        "definition": definition,
        "stateKeys": state_keys,
        "stateWarning": state_warning,
        "latestRun": latest_run,
    });
    Ok(payload.to_string())
}

/// 节点级变更的共用前置：取数 + draft 门禁。Err 即面向模型的拒绝文本。
async fn resolve_draft_plan(
    store: &GraphStore,
    workspace_id: &str,
    arguments: &Value,
) -> std::result::Result<GraphPlanRecord, String> {
    let plan = resolve_plan(store, workspace_id, arguments).await?;
    if plan.status != PLAN_DRAFT {
        return Err(format!(
            "错误：图计划当前状态为 {}，仅待确认（draft）的图可修改节点；已运行的图请读取 graph_plan_report 后用 submit_graph + inheritsFrom 提交修复图。",
            plan.status
        ));
    }
    Ok(plan)
}

/// `graph_node_update` 协议拦截：定位 draft 图目标节点 → 应用 patch（提供即
/// 整字段替换）→ 整图校验（复用 `apply_draft_definition_update`，含并发双检
/// 与广播）。校验/状态失败按可重试错误交回模型自修复。
pub(crate) async fn intercept_graph_node_update(
    db: &DispatcherDb,
    app_handle: Option<&AppHandle>,
    workspace_id: &str,
    arguments: &Value,
) -> Result<GraphNodeMutationOutcome> {
    let store = GraphStore::new(db);
    let plan = match resolve_draft_plan(&store, workspace_id, arguments).await {
        Ok(plan) => plan,
        Err(error) => return Ok(GraphNodeMutationOutcome::Rejected { error }),
    };

    let node_id = arguments
        .get("nodeId")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if node_id.is_empty() {
        return Ok(GraphNodeMutationOutcome::Rejected {
            error: "错误：graph_node_update 缺少 nodeId 参数。".to_string(),
        });
    }
    let Some(patch_value) = arguments.get("patch") else {
        return Ok(GraphNodeMutationOutcome::Rejected {
            error: "错误：graph_node_update 缺少 patch 参数。".to_string(),
        });
    };
    // deny_unknown_fields 在此处把拼错的字段名变成显式错误（静默忽略会让
    // 模型以为改成功了）。空 patch 单独拦截，给出更准确的指引。
    let patch: GraphNodePatch = serde_json::from_value(patch_value.clone()).map_err(|error| {
        anyhow::anyhow!("错误：patch 结构不合法：{error}")
    })?;
    if patch.is_empty() {
        return Ok(GraphNodeMutationOutcome::Rejected {
            error: "错误：patch 至少要提供一个要修改的字段。".to_string(),
        });
    }

    let mut definition: GraphDefinition = serde_json::from_str(&plan.definition_json)
        .map_err(|error| anyhow::anyhow!("图定义损坏（JSON 解析失败：{error}），无法更新节点"))?;
    let Some(node) = definition.nodes.iter_mut().find(|node| node.id == node_id) else {
        let available = definition
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>()
            .join("、");
        return Ok(GraphNodeMutationOutcome::Rejected {
            error: format!("错误：节点 '{node_id}' 不存在；本图可用节点：{available}。"),
        });
    };
    let changed_fields = apply_node_patch(node, patch);
    definition.normalize_ids();

    // 共享函数的 String 错误（校验问题清单/状态门禁/写入失败）按可重试错误
    // 交回模型自修复——与 submit_graph 校验失败同一闭环。
    if let Err(error) = apply_draft_definition_update(&store, app_handle, &plan, &definition).await
    {
        return Ok(GraphNodeMutationOutcome::Rejected { error });
    }

    Ok(GraphNodeMutationOutcome::Applied {
        text: format!(
            "已更新节点 {node_id} 的字段：{}。图仍为待确认（draft）状态，等待用户确认后执行。",
            changed_fields.join("、")
        ),
    })
}

/// `graph_node_add` 协议拦截：追加节点（完整定义）→ 可选 `insertBefore` 原子
/// 插入执行边中间：指定下游节点 dependsOn 中、与新节点上游重合的依赖改写为
/// 新节点（A→B 插成 A→N→B）→ 整图校验落库。
pub(crate) async fn intercept_graph_node_add(
    db: &DispatcherDb,
    app_handle: Option<&AppHandle>,
    workspace_id: &str,
    arguments: &Value,
) -> Result<GraphNodeMutationOutcome> {
    let store = GraphStore::new(db);
    let plan = match resolve_draft_plan(&store, workspace_id, arguments).await {
        Ok(plan) => plan,
        Err(error) => return Ok(GraphNodeMutationOutcome::Rejected { error }),
    };

    let Some(node_value) = arguments.get("node") else {
        return Ok(GraphNodeMutationOutcome::Rejected {
            error: "错误：graph_node_add 缺少 node 参数（须为完整节点定义）。".to_string(),
        });
    };
    let mut new_node: GraphNode = serde_json::from_value(node_value.clone())
        .map_err(|error| anyhow::anyhow!("错误：node 结构不合法：{error}"))?;
    if new_node.id.trim().is_empty() {
        return Ok(GraphNodeMutationOutcome::Rejected {
            error: "错误：node.id 不能为空。".to_string(),
        });
    }
    let insert_before = arguments
        .get("insertBefore")
        .map(|value| {
            serde_json::from_value::<Vec<String>>(value.clone()).map_err(|error| {
                anyhow::anyhow!("错误：insertBefore 必须是节点 id 数组：{error}")
            })
        })
        .transpose()?;

    let mut definition: GraphDefinition = serde_json::from_str(&plan.definition_json)
        .map_err(|error| anyhow::anyhow!("图定义损坏（JSON 解析失败：{error}），无法新增节点"))?;
    // 重复 id 检查须在 trim 后做（与全图 normalize_ids 同规则）；其余字段
    // 在 push 后由 definition.normalize_ids() 统一归一。
    new_node.id = new_node.id.trim().to_string();
    if definition.nodes.iter().any(|node| node.id == new_node.id) {
        return Ok(GraphNodeMutationOutcome::Rejected {
            error: format!(
                "错误：节点 id '{}' 已存在；请换一个 id，或用 graph_node_update 修改既有节点。",
                new_node.id
            ),
        });
    }

    // insertBefore：存在性校验 + 边改写。目标节点对本节点上游的依赖替换为
    // 本节点；与新上游无交集的目标属于模型对图结构的误解，显式拒绝优于
    // 静默无效果的「插入」。
    let mut rewritten_targets = Vec::new();
    if let Some(insert_before) = &insert_before {
        let upstream: HashSet<&str> = new_node.depends_on.iter().map(String::as_str).collect();
        for target_id in insert_before {
            let target_id = target_id.trim();
            if target_id == new_node.id {
                return Ok(GraphNodeMutationOutcome::Rejected {
                    error: "错误：insertBefore 不能包含新节点自身。".to_string(),
                });
            }
            let Some(target) = definition.nodes.iter_mut().find(|node| node.id == target_id)
            else {
                let available = definition
                    .nodes
                    .iter()
                    .map(|node| node.id.as_str())
                    .collect::<Vec<_>>()
                    .join("、");
                return Ok(GraphNodeMutationOutcome::Rejected {
                    error: format!(
                        "错误：insertBefore 引用的节点 '{target_id}' 不存在；本图可用节点：{available}。"
                    ),
                });
            };
            if !target
                .depends_on
                .iter()
                .any(|dep| upstream.contains(dep.as_str()))
            {
                return Ok(GraphNodeMutationOutcome::Rejected {
                    error: format!(
                        "错误：insertBefore 目标 '{target_id}' 不依赖新节点的任何上游（{:?}），无可接管的边；请检查图结构。",
                        new_node.depends_on
                    ),
                });
            }
            target.depends_on = target
                .depends_on
                .iter()
                .flat_map(|dep| {
                    // 命中上游：接管边；多个上游同时命中时只注入一次
                    //（下方 dedup 保序去重兜底）。
                    if upstream.contains(dep.as_str()) {
                        Some(new_node.id.clone())
                    } else {
                        Some(dep.clone())
                    }
                })
                .collect();
            rewritten_targets.push(target_id.to_string());
        }
        dedup_preserving_order(&mut definition, &rewritten_targets);
    }

    let new_node_id = new_node.id.clone();
    definition.nodes.push(new_node);
    definition.normalize_ids();

    if let Err(error) = apply_draft_definition_update(&store, app_handle, &plan, &definition).await
    {
        return Ok(GraphNodeMutationOutcome::Rejected { error });
    }

    let insert_note = if rewritten_targets.is_empty() {
        String::new()
    } else {
        format!("，并接管 {} 的上游依赖边", rewritten_targets.join("、"))
    };
    Ok(GraphNodeMutationOutcome::Applied {
        text: format!(
            "已新增节点 {new_node_id}{insert_note}。图仍为待确认（draft）状态，等待用户确认后执行。"
        ),
    })
}

/// `graph_node_delete` 协议拦截：默认拒绝删除被依赖的节点（列出下游闭包，
/// 由模型决定先改下游依赖或带 force 级联删除）；`force=true` 删除下游传递
/// 闭包，保证无悬空 dependsOn/injectStateKeys 引用。
pub(crate) async fn intercept_graph_node_delete(
    db: &DispatcherDb,
    app_handle: Option<&AppHandle>,
    workspace_id: &str,
    arguments: &Value,
) -> Result<GraphNodeMutationOutcome> {
    let store = GraphStore::new(db);
    let plan = match resolve_draft_plan(&store, workspace_id, arguments).await {
        Ok(plan) => plan,
        Err(error) => return Ok(GraphNodeMutationOutcome::Rejected { error }),
    };

    let node_id = arguments
        .get("nodeId")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if node_id.is_empty() {
        return Ok(GraphNodeMutationOutcome::Rejected {
            error: "错误：graph_node_delete 缺少 nodeId 参数。".to_string(),
        });
    }
    let force = arguments
        .get("force")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut definition: GraphDefinition = serde_json::from_str(&plan.definition_json)
        .map_err(|error| anyhow::anyhow!("图定义损坏（JSON 解析失败：{error}），无法删除节点"))?;
    if !definition.nodes.iter().any(|node| node.id == node_id) {
        let available = definition
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>()
            .join("、");
        return Ok(GraphNodeMutationOutcome::Rejected {
            error: format!("错误：节点 '{node_id}' 不存在；本图可用节点：{available}。"),
        });
    }

    // 下游闭包（依赖它的节点、依赖那些节点的节点……）。injectStateKeys 的
    // 生产者必须是拓扑祖先，因此引用本节点产出的节点全部落在闭包内——
    // 级联删除后不可能残留悬空引用（validate 兜底复核）。
    let closure = downstream_closure(&definition, node_id);
    if !closure.is_empty() && !force {
        let mut dependents = closure.iter().cloned().collect::<Vec<_>>();
        dependents.sort_unstable();
        return Ok(GraphNodeMutationOutcome::Rejected {
            error: format!(
                "错误：节点 '{node_id}' 被 {} 依赖（含传递下游），不能直接删除。请先用 graph_node_update 调整这些节点的 dependsOn，或确认后带 force=true 级联删除全部下游。",
                dependents.join("、")
            ),
        });
    }

    let mut removed = closure;
    removed.insert(node_id.to_string());
    if definition.nodes.len() <= removed.len() {
        return Ok(GraphNodeMutationOutcome::Rejected {
            error: "错误：删除后会清空图中全部节点（执行图至少保留一个节点）；如需重做整图请用 submit_graph 重新提交。".to_string(),
        });
    }
    // 按 definition 顺序列出被删节点（起点排最前），确认文本稳定可读。
    let mut removed_list = definition
        .nodes
        .iter()
        .filter(|node| removed.contains(&node.id))
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    removed_list.retain(|id| id != node_id);
    removed_list.insert(0, node_id.to_string());
    definition.nodes.retain(|node| !removed.contains(&node.id));
    definition.normalize_ids();

    if let Err(error) = apply_draft_definition_update(&store, app_handle, &plan, &definition).await
    {
        return Ok(GraphNodeMutationOutcome::Rejected { error });
    }

    let cascade_note = if removed_list.len() > 1 {
        format!("（级联删除下游：{}）", removed_list[1..].join("、"))
    } else {
        String::new()
    };
    Ok(GraphNodeMutationOutcome::Applied {
        text: format!(
            "已删除节点 {}{cascade_note}。图仍为待确认（draft）状态，等待用户确认后执行。",
            removed_list.join("、")
        ),
    })
}

/// 节点 id 的下游传递闭包：直接依赖者 + 依赖者的依赖者……（不含起点自身）。
fn downstream_closure(definition: &GraphDefinition, node_id: &str) -> HashSet<String> {
    let mut closure = HashSet::new();
    let mut frontier = vec![node_id.to_string()];
    while let Some(current) = frontier.pop() {
        for node in &definition.nodes {
            if closure.contains(&node.id) {
                continue;
            }
            if node.depends_on.iter().any(|dep| dep == &current) {
                closure.insert(node.id.clone());
                frontier.push(node.id.clone());
            }
        }
    }
    closure
}

/// insertBefore 边改写后的保序去重：多个上游同时命中时目标节点会重复出现
/// 新节点 id，逐节点收敛依赖列表。
fn dedup_preserving_order(definition: &mut GraphDefinition, targets: &[String]) {
    for target_id in targets {
        let Some(target) = definition
            .nodes
            .iter_mut()
            .find(|node| node.id.as_str() == target_id.as_str())
        else {
            continue;
        };
        let mut seen = HashSet::new();
        target.depends_on.retain(|dep| seen.insert(dep.clone()));
    }
}

/// 节点定义 patch：字段 Option 化，`Some` 即整字段替换（数组字段整体替换，
/// 不做元素级合并）。节点 id 不在 patch 面（id 是依赖引用锚点，改 id 请重提整图）。
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct GraphNodePatch {
    title: Option<String>,
    role: Option<String>,
    task: Option<String>,
    model_ref: Option<String>,
    base_tool_group: Option<BaseToolGroup>,
    depends_on: Option<Vec<String>>,
    inject_state_keys: Option<Vec<String>>,
    output_key: Option<String>,
    expected_files: Option<Vec<String>>,
    export_policy: Option<ExportPolicy>,
    use_plan_mode: Option<bool>,
}

impl GraphNodePatch {
    fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.role.is_none()
            && self.task.is_none()
            && self.model_ref.is_none()
            && self.base_tool_group.is_none()
            && self.depends_on.is_none()
            && self.inject_state_keys.is_none()
            && self.output_key.is_none()
            && self.expected_files.is_none()
            && self.export_policy.is_none()
            && self.use_plan_mode.is_none()
    }
}

/// 应用 patch 到节点，返回被修改的字段名（确认文本用）。
fn apply_node_patch(node: &mut GraphNode, patch: GraphNodePatch) -> Vec<&'static str> {
    let mut changed = Vec::new();
    if let Some(value) = patch.title {
        node.title = value;
        changed.push("title");
    }
    if let Some(value) = patch.role {
        node.role = value;
        changed.push("role");
    }
    if let Some(value) = patch.task {
        node.task = value;
        changed.push("task");
    }
    if let Some(value) = patch.model_ref {
        node.model_ref = value;
        changed.push("modelRef");
    }
    if let Some(value) = patch.base_tool_group {
        node.base_tool_group = value;
        changed.push("baseToolGroup");
    }
    if let Some(value) = patch.depends_on {
        node.depends_on = value;
        changed.push("dependsOn");
    }
    if let Some(value) = patch.inject_state_keys {
        node.inject_state_keys = value;
        changed.push("injectStateKeys");
    }
    if let Some(value) = patch.output_key {
        node.output_key = value;
        changed.push("outputKey");
    }
    if let Some(value) = patch.expected_files {
        node.expected_files = value;
        changed.push("expectedFiles");
    }
    if let Some(value) = patch.export_policy {
        node.export_policy = value;
        changed.push("exportPolicy");
    }
    if let Some(value) = patch.use_plan_mode {
        node.use_plan_mode = value;
        changed.push("usePlanMode");
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str) -> GraphNode {
        serde_json::from_value(json!({
            "id": id,
            "title": "实现",
            // modelRef 必须命中静态 ACP 目录（default/sonnet/opus/haiku），
            // 否则 node_update 成功路径会在 validate 处失败。
            "modelRef": "sonnet",
            "baseToolGroup": "read_only",
            "task": "检查实现",
            "outputKey": "result",
        }))
        .unwrap()
    }

    fn definition() -> GraphDefinition {
        let mut second = node("n2");
        // outputKey 全局唯一是校验硬约束：两节点不能共用 "result"。
        second.output_key = "result2".into();
        GraphDefinition {
            version: crate::agent::graph::types::GRAPH_DEFINITION_VERSION,
            title: "测试图".into(),
            summary: String::new(),
            state_keys: vec![],
            nodes: vec![node("n1"), second],
            inherits_from: None,
        }
    }

    fn test_db() -> crate::agent::db::DispatcherDb {
        crate::agent::db::DispatcherDb::new(
            std::env::temp_dir().join(format!("aha-graph-ops-{}.sqlite3", uuid::Uuid::new_v4())),
        )
        .unwrap()
    }

    #[test]
    fn patch_replaces_provided_fields_and_keeps_rest() {
        let mut target = node("n1");
        let patch: GraphNodePatch =
            serde_json::from_value(json!({ "task": "新任务", "usePlanMode": true })).unwrap();
        assert!(!patch.is_empty());
        let changed = apply_node_patch(&mut target, patch);
        assert_eq!(changed, vec!["task", "usePlanMode"]);
        assert_eq!(target.task, "新任务");
        assert!(target.use_plan_mode);
        // 未提供的字段保持不变。
        assert_eq!(target.title, "实现");
        assert_eq!(target.model_ref, "sonnet");
        assert_eq!(target.base_tool_group, BaseToolGroup::ReadOnly);
    }

    #[test]
    fn patch_replaces_array_wholesale() {
        let mut target = node("n1");
        let patch: GraphNodePatch =
            serde_json::from_value(json!({ "dependsOn": ["n0"], "injectStateKeys": [] }))
                .unwrap();
        apply_node_patch(&mut target, patch);
        assert_eq!(target.depends_on, vec!["n0".to_string()]);
        assert!(target.inject_state_keys.is_empty());
    }

    #[test]
    fn patch_rejects_unknown_fields_and_id() {
        // id 不在 patch 面：deny_unknown_fields 拒绝。
        assert!(serde_json::from_value::<GraphNodePatch>(json!({ "id": "n2" })).is_err());
        assert!(
            serde_json::from_value::<GraphNodePatch>(json!({ "specialTools": [] })).is_err()
        );
        // 空 patch 可解析但 is_empty。
        let empty: GraphNodePatch = serde_json::from_value(json!({})).unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn graph_get_returns_definition_and_latest_run() {
        let db = test_db();
        let store = GraphStore::new(&db);
        let plan = store
            .create_plan_async("w", &definition(), "需求", "{}")
            .await
            .unwrap();
        let run = store.create_run_async(&plan.id).await.unwrap();

        let text = build_graph_get(&db, "w", &json!({}))
            .await
            .unwrap();
        let payload: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(payload["planId"], plan.id.as_str());
        assert_eq!(payload["status"], "running");
        assert_eq!(payload["definition"]["nodes"].as_array().unwrap().len(), 2);
        assert_eq!(payload["stateKeys"], json!([]));
        assert_eq!(payload["latestRun"]["runId"], run.id.as_str());

        // 无图会话返回说明性文本（不是「错误：」前缀）。
        let empty = build_graph_get(&db, "no-session", &json!({})).await.unwrap();
        assert!(empty.contains("还没有执行图"));
        // 显式 planId 拼错返回「错误：」引导。
        let wrong = build_graph_get(&db, "w", &json!({ "planId": "nope" }))
            .await
            .unwrap();
        assert!(wrong.contains("错误："));
    }

    #[tokio::test]
    async fn node_update_applies_patch_and_persists() {
        let db = test_db();
        let store = GraphStore::new(&db);
        let plan = store
            .create_plan_async("w", &definition(), "需求", "{}")
            .await
            .unwrap();

        let outcome = intercept_graph_node_update(
            &db,
            None,
            "w",
            &json!({ "nodeId": "n1", "patch": { "task": "重写后的任务" } }),
        )
        .await
        .unwrap();
        match outcome {
            GraphNodeMutationOutcome::Applied { text } => {
                assert!(text.contains("task"));
            }
            GraphNodeMutationOutcome::Rejected { error } => panic!("应成功：{error}"),
        }
        let updated = store.get_plan_async(&plan.id).await.unwrap().unwrap();
        let definition: GraphDefinition =
            serde_json::from_str(&updated.definition_json).unwrap();
        assert_eq!(definition.nodes[0].task, "重写后的任务");
        // 未提供字段保持不变；其他节点不受影响。
        assert_eq!(definition.nodes[0].title, "实现");
        assert_eq!(definition.nodes[1].task, "检查实现");
    }

    #[tokio::test]
    async fn node_update_rejects_unknown_node_and_lists_available() {
        let db = test_db();
        let store = GraphStore::new(&db);
        store
            .create_plan_async("w", &definition(), "需求", "{}")
            .await
            .unwrap();
        let outcome = intercept_graph_node_update(
            &db,
            None,
            "w",
            &json!({ "nodeId": "nx", "patch": { "task": "x" } }),
        )
        .await
        .unwrap();
        match outcome {
            GraphNodeMutationOutcome::Rejected { error } => {
                assert!(error.contains("不存在"));
                assert!(error.contains("n1") && error.contains("n2"));
            }
            GraphNodeMutationOutcome::Applied { text } => panic!("应拒绝：{text}"),
        }
    }

    #[tokio::test]
    async fn node_update_rejects_non_draft_plan_and_bad_patch() {
        let db = test_db();
        let store = GraphStore::new(&db);
        let plan = store
            .create_plan_async("w", &definition(), "需求", "{}")
            .await
            .unwrap();
        store.create_run_async(&plan.id).await.unwrap();

        let non_draft = intercept_graph_node_update(
            &db,
            None,
            "w",
            &json!({ "nodeId": "n1", "patch": { "task": "x" } }),
        )
        .await
        .unwrap();
        assert!(matches!(
            non_draft,
            GraphNodeMutationOutcome::Rejected { .. }
        ));
        if let GraphNodeMutationOutcome::Rejected { error } = non_draft {
            assert!(error.contains("draft") || error.contains("修复图"));
        }

        // 校验失败（依赖不存在的节点）交回可重试错误。
        let plan2 = store
            .create_plan_async("w2", &definition(), "需求", "{}")
            .await
            .unwrap();
        let _ = plan2;
        let invalid = intercept_graph_node_update(
            &db,
            None,
            "w2",
            &json!({ "nodeId": "n1", "patch": { "dependsOn": ["ghost"] } }),
        )
        .await
        .unwrap();
        match invalid {
            GraphNodeMutationOutcome::Rejected { error } => {
                assert!(error.contains("错误") || !error.is_empty())
            }
            GraphNodeMutationOutcome::Applied { text } => panic!("应拒绝：{text}"),
        }

        // 空 patch 拒绝。
        let empty = intercept_graph_node_update(
            &db,
            None,
            "w2",
            &json!({ "nodeId": "n1", "patch": {} }),
        )
        .await
        .unwrap();
        assert!(matches!(empty, GraphNodeMutationOutcome::Rejected { .. }));
    }

    /// 链式图：n1 ← n2 ← n3（n2 依赖 n1，n3 依赖 n2）。
    fn chain_definition() -> GraphDefinition {
        let mut n2 = node("n2");
        n2.depends_on = vec!["n1".into()];
        n2.output_key = "result2".into();
        let mut n3 = node("n3");
        n3.depends_on = vec!["n2".into()];
        n3.output_key = "result3".into();
        GraphDefinition {
            version: crate::agent::graph::types::GRAPH_DEFINITION_VERSION,
            title: "链式图".into(),
            summary: String::new(),
            state_keys: vec![],
            nodes: vec![node("n1"), n2, n3],
            inherits_from: None,
        }
    }

    #[tokio::test]
    async fn node_add_inserts_between_and_appends() {
        let db = test_db();
        let store = GraphStore::new(&db);
        let plan = store
            .create_plan_async("w", &chain_definition(), "需求", "{}")
            .await
            .unwrap();

        // 中间插入：n_new 依赖 n1，接管 n2 对 n1 的边 → n1←n_new←n2。
        let outcome = intercept_graph_node_add(
            &db,
            None,
            "w",
            &json!({
                "node": {
                    "id": "n_mid", "title": "中间步骤", "modelRef": "sonnet",
                    "baseToolGroup": "read_only", "task": "中间处理", "outputKey": "mid",
                    "dependsOn": ["n1"]
                },
                "insertBefore": ["n2"],
            }),
        )
        .await
        .unwrap();
        match outcome {
            GraphNodeMutationOutcome::Applied { text } => assert!(text.contains("n_mid")),
            GraphNodeMutationOutcome::Rejected { error } => panic!("应成功：{error}"),
        }
        let updated = store.get_plan_async(&plan.id).await.unwrap().unwrap();
        let definition: GraphDefinition =
            serde_json::from_str(&updated.definition_json).unwrap();
        let by_id = |id: &str| {
            definition
                .nodes
                .iter()
                .find(|node| node.id == id)
                .unwrap_or_else(|| panic!("节点 {id} 应存在"))
        };
        assert_eq!(by_id("n_mid").depends_on, vec!["n1".to_string()]);
        assert_eq!(by_id("n2").depends_on, vec!["n_mid".to_string()]);
        // n3 不受影响。
        assert_eq!(by_id("n3").depends_on, vec!["n2".to_string()]);

        // 尾部追加（无 insertBefore）：挂在 n3 下游。
        let appended = intercept_graph_node_add(
            &db,
            None,
            "w",
            &json!({
                "node": {
                    "id": "n_tail", "title": "收尾", "modelRef": "sonnet",
                    "baseToolGroup": "read_only", "task": "收尾任务", "outputKey": "tail",
                    "dependsOn": ["n3"]
                },
            }),
        )
        .await
        .unwrap();
        assert!(matches!(appended, GraphNodeMutationOutcome::Applied { .. }));
    }

    #[tokio::test]
    async fn node_add_rejects_duplicate_bad_insert_and_cycles() {
        let db = test_db();
        let store = GraphStore::new(&db);
        store
            .create_plan_async("w", &chain_definition(), "需求", "{}")
            .await
            .unwrap();

        // 重复 id。
        let duplicate = intercept_graph_node_add(
            &db,
            None,
            "w",
            &json!({
                "node": {
                    "id": "n1", "title": "重复", "modelRef": "sonnet",
                    "baseToolGroup": "read_only", "task": "x", "outputKey": "dup"
                },
            }),
        )
        .await
        .unwrap();
        match duplicate {
            GraphNodeMutationOutcome::Rejected { error } => assert!(error.contains("已存在")),
            GraphNodeMutationOutcome::Applied { text } => panic!("应拒绝：{text}"),
        }

        // insertBefore 指向不存在节点。
        let ghost = intercept_graph_node_add(
            &db,
            None,
            "w",
            &json!({
                "node": {
                    "id": "n_x", "title": "x", "modelRef": "sonnet",
                    "baseToolGroup": "read_only", "task": "x", "outputKey": "x",
                    "dependsOn": ["n1"]
                },
                "insertBefore": ["ghost"],
            }),
        )
        .await
        .unwrap();
        match ghost {
            GraphNodeMutationOutcome::Rejected { error } => assert!(error.contains("不存在")),
            GraphNodeMutationOutcome::Applied { text } => panic!("应拒绝：{text}"),
        }

        // insertBefore 目标不依赖新节点的上游：无可接管的边。
        let no_edge = intercept_graph_node_add(
            &db,
            None,
            "w",
            &json!({
                "node": {
                    "id": "n_y", "title": "y", "modelRef": "sonnet",
                    "baseToolGroup": "read_only", "task": "y", "outputKey": "y",
                    "dependsOn": ["n3"]
                },
                "insertBefore": ["n2"],
            }),
        )
        .await
        .unwrap();
        match no_edge {
            GraphNodeMutationOutcome::Rejected { error } => assert!(error.contains("无可接管")),
            GraphNodeMutationOutcome::Applied { text } => panic!("应拒绝：{text}"),
        }

        // 接管边成环由整图校验兜底：n4 依赖 n2 与 n3，接管 n3 对 n2 的边后
        // n3→n4 而 n4→n3，形成环。
        let cyclic = intercept_graph_node_add(
            &db,
            None,
            "w",
            &json!({
                "node": {
                    "id": "n_z", "title": "z", "modelRef": "sonnet",
                    "baseToolGroup": "read_only", "task": "z", "outputKey": "z",
                    "dependsOn": ["n2", "n3"]
                },
                "insertBefore": ["n3"],
            }),
        )
        .await
        .unwrap();
        match cyclic {
            GraphNodeMutationOutcome::Rejected { error } => assert!(error.contains("校验")),
            GraphNodeMutationOutcome::Applied { text } => panic!("应拒绝：{text}"),
        }
    }

    #[tokio::test]
    async fn node_delete_leaf_rejects_dependent_and_cascades() {
        let db = test_db();
        let store = GraphStore::new(&db);
        let plan = store
            .create_plan_async("w", &chain_definition(), "需求", "{}")
            .await
            .unwrap();

        // 叶子（无下游）直接删除。
        let leaf = intercept_graph_node_delete(&db, None, "w", &json!({ "nodeId": "n3" }))
            .await
            .unwrap();
        match leaf {
            GraphNodeMutationOutcome::Applied { text } => assert!(text.contains("n3")),
            GraphNodeMutationOutcome::Rejected { error } => panic!("应成功：{error}"),
        }

        // n1 被传递下游依赖：默认拒绝并列出全部下游。
        let guarded = intercept_graph_node_delete(&db, None, "w", &json!({ "nodeId": "n1" }))
            .await
            .unwrap();
        match guarded {
            GraphNodeMutationOutcome::Rejected { error } => {
                assert!(error.contains("n2"));
            }
            GraphNodeMutationOutcome::Applied { text } => panic!("应拒绝：{text}"),
        }

        // force=true：级联删除 n1 与其全部下游（此时剩 n1、n2，删后应为空 → 拒绝）。
        let wipe = intercept_graph_node_delete(
            &db,
            None,
            "w",
            &json!({ "nodeId": "n1", "force": true }),
        )
        .await
        .unwrap();
        match wipe {
            GraphNodeMutationOutcome::Rejected { error } => assert!(error.contains("至少保留")),
            GraphNodeMutationOutcome::Applied { text } => panic!("应拒绝：{text}"),
        }
        let _ = plan;

        // 带独立节点重建图后 force 级联：n1 闭包 {n2}，独立节点 n_iso 保留。
        let mut branched = chain_definition();
        branched.nodes.push(node("n_iso"));
        let plan2 = store
            .create_plan_async("w2", &branched, "需求", "{}")
            .await
            .unwrap();
        let cascaded = intercept_graph_node_delete(
            &db,
            None,
            "w2",
            &json!({ "nodeId": "n1", "force": true }),
        )
        .await
        .unwrap();
        match cascaded {
            GraphNodeMutationOutcome::Applied { text } => {
                assert!(text.contains("n1"));
                assert!(text.contains("级联"));
            }
            GraphNodeMutationOutcome::Rejected { error } => panic!("应成功：{error}"),
        }
        let updated = store
            .get_plan_async(&plan2.id)
            .await
            .unwrap()
            .unwrap();
        let definition: GraphDefinition =
            serde_json::from_str(&updated.definition_json).unwrap();
        let ids: Vec<&str> = definition.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["n_iso"], "级联删除后只应剩独立节点");
    }

    #[test]
    fn downstream_closure_handles_diamond() {
        // 菱形：n2、n3 都依赖 n1，n4 依赖 n2 与 n3——n1 的闭包 = {n2, n3, n4}。
        let mut n2 = node("n2");
        n2.depends_on = vec!["n1".into()];
        let mut n3 = node("n3");
        n3.depends_on = vec!["n1".into()];
        let mut n4 = node("n4");
        n4.depends_on = vec!["n2".into(), "n3".into()];
        let definition = GraphDefinition {
            version: crate::agent::graph::types::GRAPH_DEFINITION_VERSION,
            title: "菱形".into(),
            summary: String::new(),
            state_keys: vec![],
            nodes: vec![node("n1"), n2, n3, n4],
            inherits_from: None,
        };
        let closure = downstream_closure(&definition, "n1");
        let mut ids = closure.iter().cloned().collect::<Vec<_>>();
        ids.sort_unstable();
        assert_eq!(ids, vec!["n2".to_string(), "n3".to_string(), "n4".to_string()]);
        // 中游节点的闭包不含平行分支。
        let closure = downstream_closure(&definition, "n2");
        assert_eq!(closure, vec!["n4".to_string()].into_iter().collect());
    }
}

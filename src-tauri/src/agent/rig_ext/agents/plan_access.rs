//! 编排器协议层与图运行共用的图计划取数口径：planId 解析校验、最近运行
//! 选择、需求快照兜底。`graph_plan_report` / `graph_result_read` / `graph_get` /
//! `graph_node_{update,add,delete}` 拦截器与图运行/重新验收共用同一份，
//! 防止各处自行演化出取数与兜底口径漂移。

use anyhow::Result;
use serde_json::Value;

use crate::agent::db::DispatcherDb;
use crate::agent::graph::types::{GraphPlanRecord, GraphRunSummary};
use crate::agent::graph::GraphStore;

/// 解析 planId 参数并校验归属：可选 planId（缺省取会话最近图计划）。
/// 内层 Err 为面向模型的引导文本（无计划 / planId 不存在 / 跨会话误用），
/// 作为正常工具结果返回让模型自行决策；外层 Err 是存储读取失败等基础设施
/// 错误，由调用方按内部错误处理。
pub(crate) async fn resolve_session_plan(
    store: &GraphStore,
    workspace_id: &str,
    arguments: &Value,
) -> Result<std::result::Result<GraphPlanRecord, String>> {
    let plan_id_arg = arguments
        .get("planId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    let explicit_plan_id = plan_id_arg.is_some();
    let plan = match plan_id_arg {
        Some(plan_id) => store.get_plan_async(&plan_id).await?,
        None => store.latest_plan_for_workspace_async(workspace_id).await?,
    };
    let Some(plan) = plan else {
        // 显式给了 planId 却查不到（拼写错误/已清理）时，返回「错误：」前缀的
        // 明确提示引导模型纠正；「从未出图」的说明性文本只适用于未传 planId
        // 的场景（审查项 G8-17）。
        return Ok(Err(if explicit_plan_id {
            "错误：指定的 planId 不存在或已被清理；可不带 planId 取会话最近的图计划，或修正 planId 后重试。"
                .to_string()
        } else {
            "当前会话还没有执行图。若任务复杂，请先探索项目后用 submit_graph 出图。".to_string()
        }));
    };
    // workspace 校验只对显式传 planId 的路径有意义：未传 planId 时
    // latest_plan_for_workspace_async 本身已按 workspace_id 过滤。
    if plan.workspace_id != workspace_id {
        return Ok(Err("错误：指定的 planId 不属于当前会话。".to_string()));
    }
    Ok(Ok(plan))
}

/// 报告/结果/感知头的运行选择：优先与 latest_run_id 一致的 run，找不到再退回
/// runs.first()（attempt_no DESC）。各视图必须同源，防止报告头与节点明细静默
/// 来自不同运行（审查项 G8-18）。
pub(crate) fn latest_run_of(plan: &GraphPlanRecord) -> Option<&GraphRunSummary> {
    match plan.latest_run_id.as_deref() {
        Some(run_id) => plan
            .runs
            .iter()
            .find(|run| run.id == run_id)
            .or_else(|| plan.runs.first()),
        None => plan.runs.first(),
    }
}

/// 需求快照兜底：v3 起需求以图提交时快照为准；快照为空的旧数据兜底取会话
/// 最新用户消息。运行验收与重新验收必须同源，防止两处兜底口径漂移。
pub(crate) async fn resolve_user_requirement(
    db: &DispatcherDb,
    workspace_id: &str,
    requirement: &str,
) -> String {
    let requirement = requirement.trim();
    if !requirement.is_empty() {
        return requirement.to_string();
    }
    db.get_latest_user_message_content_async(workspace_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}

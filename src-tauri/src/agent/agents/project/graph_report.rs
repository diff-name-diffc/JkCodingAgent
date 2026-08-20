//! `graph_plan_report` 协议拦截：读取图计划最近一次运行的紧凑报告
//! （验收结论、各节点状态/输出摘要/错误、共享 state 键），作为工具结果
//! 返回给编排器——支撑「失败 → 读报告 → 提交 inheritsFrom 修复图」的反思闭环。
//!
//! 与 submit_graph 拦截不同：本拦截不收口本轮，模型拿到报告后继续决策。

use anyhow::Result;
use serde_json::Value;

use crate::agent::db::DispatcherDb;
use crate::agent::graph::types::{
    GraphNodeRunRecord, NODE_FAILED, NODE_PHASE_CACHED, NODE_SUCCEEDED, PLAN_RUNNING,
    RUN_MODE_FULL, RUN_MODE_RESUME, VERDICT_FAIL, VERDICT_PARTIAL, VERDICT_PASS,
};
use crate::agent::graph::GraphStore;
use crate::agent::llm::RequestedToolCall;

use super::OrchestratorAgent;

/// 报告节点输出摘要的最大字符数。
const OUTPUT_PREVIEW_CHARS: usize = 400;
const ERROR_PREVIEW_CHARS: usize = 300;

impl OrchestratorAgent {
    /// graph_plan_report 拦截：返回运行报告文本（永不收口）。
    /// 无图计划/运行记录时返回说明性文本，模型可据此决定直接答复或重新出图。
    pub(super) async fn intercept_graph_plan_report(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        tool_call: &RequestedToolCall,
    ) -> Result<String> {
        let store = GraphStore::new(db);
        let plan_id_arg = tool_call
            .arguments
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
            // 显式给了 planId 却查不到（拼写错误/已清理）时，返回「错误：」前缀
            // 的明确提示引导模型纠正 planId；「从未出图」的说明性文本只适用于
            // 未传 planId 的场景（审查项 G8-17）。
            return Ok(if explicit_plan_id {
                "错误：指定的 plan_id 不存在或已被清理，请重新确认后查询。".to_string()
            } else {
                "当前会话还没有提交过执行图。若任务复杂，请先探索项目后用 submit_graph 出图。"
                    .to_string()
            });
        };
        // workspace 校验只对显式传 planId 的路径有意义：未传 planId 时
        // latest_plan_for_workspace_async 本身已按 workspace_id 过滤。
        if plan.workspace_id != workspace_id {
            return Ok("错误：指定的 plan_id 不属于当前会话。".to_string());
        }
        // 报告头运行选择：优先与 latest_run_id 一致的 run（节点明细记录即按该
        // run 加载），找不到再退回 runs.first()（attempt_no DESC）；随后显式
        // 校验节点明细同源，防止 store 排序/维护逻辑变更后报告头与节点明细
        // 静默来自不同运行（审查项 G8-18）。
        let latest_run = match plan.latest_run_id.as_deref() {
            Some(run_id) => plan
                .runs
                .iter()
                .find(|run| run.id == run_id)
                .or_else(|| plan.runs.first()),
            None => plan.runs.first(),
        };
        let Some(latest_run) = latest_run else {
            return Ok(format!(
                "执行图《{}》（plan_id={}，状态 {}）尚未运行过。{}",
                plan.title,
                plan.id,
                plan.status,
                plan.summary.trim()
            ));
        };
        let total_node_runs = plan.node_runs.len();
        let node_runs: Vec<&GraphNodeRunRecord> = plan
            .node_runs
            .iter()
            .filter(|record| record.run_id == latest_run.id)
            .collect();
        let node_run_mismatch = node_runs.len() != total_node_runs;
        Ok(build_report(
            &plan.title,
            &plan.id,
            &plan.status,
            latest_run.attempt_no,
            &latest_run.mode,
            &latest_run.status,
            &latest_run.verdict_status,
            &latest_run.verdict_reason,
            &node_runs,
            &plan.state_json,
            node_run_mismatch,
        ))
    }
}

#[allow(clippy::too_many_arguments)]
fn build_report(
    title: &str,
    plan_id: &str,
    plan_status: &str,
    attempt_no: i64,
    mode: &str,
    run_status: &str,
    verdict_status: &str,
    verdict_reason: &str,
    node_runs: &[&GraphNodeRunRecord],
    state_json: &str,
    node_run_mismatch: bool,
) -> String {
    let mut lines = vec![format!(
        "执行图《{title}》（plan_id={plan_id}）计划状态：{plan_status}"
    )];
    // 显式枚举运行模式：未来新增模式时落入「未知模式」而不是被静默描述为完整执行。
    let mode_note = match mode {
        RUN_MODE_FULL => "完整执行",
        RUN_MODE_RESUME => "断点续跑",
        _ => "未知模式",
    };
    // 「尚未验收」（运行中）与「未能验收」（unknown/空串：验收不可用或失败）区分开；
    // 空串与 unknown 语义等价（读取层已归一为 unknown），这里一并兜底防御。
    let verdict = match verdict_status {
        VERDICT_PASS => "验收通过".to_string(),
        VERDICT_PARTIAL => "部分达成".to_string(),
        VERDICT_FAIL => "验收未通过".to_string(),
        _ => {
            if run_status == PLAN_RUNNING {
                "尚未验收".to_string()
            } else {
                "未能验收".to_string()
            }
        }
    };
    lines.push(format!(
        "最近运行：第 {attempt_no} 次（{mode_note}），运行状态 {run_status}，验收：{verdict}"
    ));
    if !verdict_reason.trim().is_empty() {
        lines.push(format!("验收理由：{}", verdict_reason.trim()));
    }
    lines.push("节点明细：".to_string());
    if node_run_mismatch {
        // 防御性告警：节点明细与报告头运行不同源（正常不应发生），
        // 已按报告头运行过滤，避免把其他运行的明细混入本报告。
        lines.push(
            "警告：部分节点明细与最近运行不一致，已按最近运行过滤；节点详情请以图面板为准。"
                .to_string(),
        );
    }
    for record in node_runs {
        let cached_note = if record.phase == NODE_PHASE_CACHED {
            "（续跑复用）"
        } else {
            ""
        };
        match record.status.as_str() {
            NODE_SUCCEEDED => {
                let summary =
                    crate::agent::graph::input::extract_summary_section(&record.output_text)
                        .unwrap_or_else(|| record.output_text.clone());
                let summary: String = summary.chars().take(OUTPUT_PREVIEW_CHARS).collect();
                lines.push(format!(
                    "- [{}] 成功{cached_note}：{summary}",
                    record.node_id
                ));
            }
            NODE_FAILED => {
                let error: String = record
                    .error_text
                    .as_deref()
                    .unwrap_or("未知错误")
                    .chars()
                    .take(ERROR_PREVIEW_CHARS)
                    .collect();
                lines.push(format!("- [{}] 失败：{error}", record.node_id));
            }
            other => {
                lines.push(format!("- [{}] {other}{cached_note}", record.node_id));
            }
        }
    }
    // state 解析失败不能静默吞掉：模型据报告决定是否提交 inheritsFrom 修复图，
    // 丢失 state 键信息会导致修复图 injectStateKeys 无从引用。
    // 防御（审查项 G8-16）：空串/纯空白等价于无 state 键，静默跳过不告警，
    // 避免与真实损坏数据混淆、误导模型放弃可用的 inheritsFrom 修复。
    if !state_json.trim().is_empty() {
        match serde_json::from_str::<Value>(state_json) {
            Ok(value) => {
                let state_keys = value
                    .as_object()
                    .map(|map| map.keys().cloned().collect::<Vec<_>>())
                    .unwrap_or_default();
                if !state_keys.is_empty() {
                    lines.push(format!("共享 state 键：{}", state_keys.join("、")));
                    lines.push("提示：修复图可通过 inheritsFrom 继承本计划的共享 state，并用 injectStateKeys 引用上述键；失败节点之外的成功成果无需重做。".to_string());
                }
            }
            Err(error) => {
                lines.push(format!("警告：共享 state 解析失败（{error}），无法列出可用 state 键；修复图请谨慎使用 inheritsFrom。"));
            }
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::build_report;
    use crate::agent::graph::types::GraphNodeRunRecord;

    fn node_run(node_id: &str, run_id: &str, status: &str) -> GraphNodeRunRecord {
        GraphNodeRunRecord {
            run_id: run_id.to_string(),
            plan_id: "plan-1".to_string(),
            node_id: node_id.to_string(),
            status: status.to_string(),
            phase: String::new(),
            model_ref: String::new(),
            model_label: String::new(),
            model_category: String::new(),
            base_tool_group: String::new(),
            special_tools_json: String::new(),
            input_text: String::new(),
            output_text: String::new(),
            error_text: None,
            started_at: None,
            finished_at: None,
            duration_ms: None,
            usage_json: String::new(),
            affected_files: Vec::new(),
            tool_call_count: 0,
            retry_count: 0,
        }
    }

    fn report_with_state(state_json: &str) -> String {
        let runs = vec![node_run("n1", "run-1", "succeeded")];
        let refs = runs.iter().collect::<Vec<_>>();
        build_report(
            "测试图",
            "plan-1",
            "completed",
            1,
            "full",
            "completed",
            "pass",
            "",
            &refs,
            state_json,
            false,
        )
    }

    #[test]
    fn empty_state_json_produces_no_warning() {
        let report = report_with_state("");
        assert!(
            !report.contains("解析失败"),
            "空 state_json 不应触发解析失败告警：{report}"
        );
        assert!(!report.contains("共享 state 键"));
        let blank = report_with_state("   ");
        assert!(!blank.contains("解析失败"));
    }

    #[test]
    fn corrupted_state_json_warns() {
        let report = report_with_state("{not json");
        assert!(report.contains("警告：共享 state 解析失败"));
    }

    #[test]
    fn valid_state_lists_keys() {
        let report = report_with_state(r#"{"api_design": "x", "test_plan": "y"}"#);
        assert!(report.contains("共享 state 键："));
        assert!(report.contains("api_design"));
        assert!(report.contains("test_plan"));
    }

    #[test]
    fn mismatch_flag_adds_warning() {
        let runs = vec![node_run("n1", "run-1", "succeeded")];
        let refs = runs.iter().collect::<Vec<_>>();
        let report = build_report(
            "测试图",
            "plan-1",
            "completed",
            1,
            "full",
            "completed",
            "pass",
            "",
            &refs,
            "{}",
            true,
        );
        assert!(report.contains("部分节点明细与最近运行不一致"));
    }
}

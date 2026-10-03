//! `graph_plan_report` / `graph_result_read` 协议拦截：前者的图计划最近一次
//! 运行的紧凑报告（验收结论、各节点状态/输出摘要/错误、共享 state 键），后者
//! 的结构化执行结果（结果类型、结论 md 全文、修改文件清单），作为工具结果
//! 返回给编排器——支撑「失败 → 读报告」「审查完成 → 读结果 → 规划修复/
//! 后续图」的反思闭环。
//!
//! 与 submit_graph 拦截不同：两者都不收口本轮，模型拿到内容后继续决策。

use anyhow::Result;
use serde_json::Value;

use super::plan_access::{latest_run_of, resolve_session_plan};
use crate::agent::db::DispatcherDb;
use crate::agent::graph::types::{
    GraphNodeRunRecord, GraphRunResult, NODE_FAILED, NODE_PHASE_CACHED, NODE_SUCCEEDED,
    PLAN_RUNNING, RESULT_KIND_EDIT, RESULT_KIND_REVIEW, RUN_MODE_FULL, RUN_MODE_RESUME,
    VERDICT_FAIL, VERDICT_PARTIAL, VERDICT_PASS,
};
use crate::agent::graph::GraphStore;

/// 报告节点输出摘要的最大字符数。
const OUTPUT_PREVIEW_CHARS: usize = 400;
const ERROR_PREVIEW_CHARS: usize = 300;
/// graph_result_read 结论全文回读的截断上限。结论（问题清单/执行总结）是
/// 模型规划后续图的依据，必须尽量保真——上限远大于报告的节点摘要；
/// 超限截断并注明完整版位置。
const CONCLUSION_MAX_CHARS: usize = 20_000;
/// 修改文件清单的列出上限（与运行回执 receipt 的 40 个口径一致）。
const MODIFIED_FILES_LIMIT: usize = 40;

/// 验收结论文案。「尚未验收」（运行中）与「未能验收」（unknown/空串）区分开；
/// 空串与 unknown 语义等价（读取层已归一为 unknown），一并兜底防御。
fn verdict_label(verdict_status: &str, run_status: &str) -> String {
    match verdict_status {
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
    }
}

/// graph_plan_report 拦截：返回运行报告文本（永不收口）。
/// 无图计划/运行记录时返回说明性文本，模型可据此决定直接答复或重新出图。
pub(crate) async fn build_plan_report(
    db: &DispatcherDb,
    workspace_id: &str,
    arguments: &Value,
) -> Result<String> {
    let store = GraphStore::new(db);
    let plan = match resolve_session_plan(&store, workspace_id, arguments).await? {
        Ok(plan) => plan,
        Err(guidance) => return Ok(guidance),
    };
    // 报告头运行选择：优先与 latest_run_id 一致的 run（节点明细记录即按该
    // run 加载），找不到再退回 runs.first()（attempt_no DESC）；随后显式
    // 校验节点明细同源，防止 store 排序/维护逻辑变更后报告头与节点明细
    // 静默来自不同运行（审查项 G8-18）。
    let Some(latest_run) = latest_run_of(&plan) else {
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

/// graph_result_read 拦截：返回执行结果文本（永不收口）。结论 md 来自 run
/// 收尾的结构化落库（v11），完整回读供模型据结论规划修复/后续图。
pub(crate) async fn build_result_read(
    db: &DispatcherDb,
    workspace_id: &str,
    arguments: &Value,
) -> Result<String> {
    let store = GraphStore::new(db);
    let plan = match resolve_session_plan(&store, workspace_id, arguments).await? {
        Ok(plan) => plan,
        Err(guidance) => return Ok(guidance),
    };
    let Some(latest_run) = latest_run_of(&plan) else {
        return Ok(format!(
            "执行图《{}》（plan_id={}，状态 {}）尚未运行过，还没有执行结果。{}",
            plan.title,
            plan.id,
            plan.status,
            plan.summary.trim()
        ));
    };
    Ok(build_result_text(
        &plan.title,
        &plan.id,
        latest_run.attempt_no,
        &latest_run.status,
        &latest_run.verdict_status,
        &latest_run.verdict_reason,
        latest_run.result.as_ref(),
    ))
}

/// 执行结果文本（纯函数）：结果类型头行 + 验收理由 + 结论全文（截断上限
/// CONCLUSION_MAX_CHARS）+ 修改文件清单。无结果（历史运行/取消）与结论缺失
/// （汇总节点未成功）各有降级指引。
fn build_result_text(
    title: &str,
    plan_id: &str,
    attempt_no: i64,
    run_status: &str,
    verdict_status: &str,
    verdict_reason: &str,
    result: Option<&GraphRunResult>,
) -> String {
    let verdict = verdict_label(verdict_status, run_status);
    let Some(result) = result else {
        return format!(
            "执行图《{title}》（plan_id={plan_id}）第 {attempt_no} 次运行没有结构化执行结果\
             （结果记录上线前的历史运行或被取消的运行）。验收：{verdict}。\n\
             需要节点级成败与失败原因请改用 graph_plan_report。"
        );
    };
    let kind_label = match result.result_kind.as_str() {
        RESULT_KIND_EDIT => "执行结果（编辑写入类）",
        RESULT_KIND_REVIEW => "审查报告（调研审查类）",
        _ => "未知类型",
    };
    let mut lines = vec![format!(
        "执行图《{title}》（plan_id={plan_id}）执行结果：第 {attempt_no} 次运行 · {kind_label} · 验收：{verdict}"
    )];
    if !verdict_reason.trim().is_empty() {
        lines.push(format!("验收理由：{}", verdict_reason.trim()));
    }
    match result.conclusion_md.as_deref().map(str::trim) {
        Some(conclusion) if !conclusion.is_empty() => {
            let source = match result.conclusion_node_id.as_deref() {
                Some(node_id) => format!("结论（来自汇总节点 {node_id}）："),
                None => "结论：".to_string(),
            };
            let total = conclusion.chars().count();
            if total > CONCLUSION_MAX_CHARS {
                let head: String = conclusion.chars().take(CONCLUSION_MAX_CHARS).collect();
                lines.push(source);
                lines.push(head);
                lines.push(format!(
                    "…（结论共 {total} 字符，已截断；完整内容见执行图面板的执行结果视图）"
                ));
            } else {
                lines.push(source);
                lines.push(conclusion.to_string());
            }
        }
        _ => {
            lines.push("结论：汇总节点未成功产出结论文本。".to_string());
            lines.push(
                "各节点输出摘要与失败原因可用 graph_plan_report 查看；完整节点输出可在图面板点开节点查看。"
                    .to_string(),
            );
        }
    }
    if !result.modified_files.is_empty() {
        let total = result.modified_files.len();
        let listed: Vec<&str> = result
            .modified_files
            .iter()
            .take(MODIFIED_FILES_LIMIT)
            .map(String::as_str)
            .collect();
        lines.push(format!("修改文件（{total} 个）：{}", listed.join("、")));
        if total > MODIFIED_FILES_LIMIT {
            lines.push(format!("…另有 {} 个文件未列出", total - MODIFIED_FILES_LIMIT));
        }
    }
    lines.join("\n")
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
    let verdict = verdict_label(verdict_status, run_status);
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
    use super::{build_report, build_result_text};
    use crate::agent::graph::types::{
        GraphNodeRunRecord, GraphRunResult, RESULT_KIND_EDIT, RESULT_KIND_REVIEW,
    };

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
        let runs = [node_run("n1", "run-1", "succeeded")];
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
        let runs = [node_run("n1", "run-1", "succeeded")];
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

    fn result_of(
        kind: &str,
        conclusion_node_id: Option<&str>,
        conclusion_md: Option<&str>,
        modified_files: &[&str],
    ) -> GraphRunResult {
        GraphRunResult {
            conclusion_node_id: conclusion_node_id.map(str::to_string),
            conclusion_md: conclusion_md.map(str::to_string),
            result_kind: kind.to_string(),
            modified_files: modified_files.iter().map(|file| file.to_string()).collect(),
        }
    }

    #[test]
    fn result_text_carries_full_conclusion_and_files() {
        let result = result_of(
            RESULT_KIND_REVIEW,
            Some("summary"),
            Some("## 审查结论\n- 问题 A（P0）：连接池未释放\n- 问题 B：缺少超时"),
            &["src/a.rs", "docs/report.md"],
        );
        let text = build_result_text("审查图", "plan-9", 2, "completed", "pass", "产出完整", Some(&result));
        assert!(text.contains("审查报告（调研审查类）"));
        assert!(text.contains("第 2 次运行"));
        assert!(text.contains("验收：验收通过"));
        assert!(text.contains("验收理由：产出完整"));
        // 结论全文保真（不经摘要/压缩）。
        assert!(text.contains("结论（来自汇总节点 summary）："));
        assert!(text.contains("- 问题 A（P0）：连接池未释放"));
        assert!(text.contains("- 问题 B：缺少超时"));
        assert!(text.contains("修改文件（2 个）：src/a.rs、docs/report.md"));
    }

    #[test]
    fn result_text_edit_kind_label() {
        let result = result_of(RESULT_KIND_EDIT, Some("n3"), Some("## 执行总结\n完成"), &["src/a.rs"]);
        let text = build_result_text("改造图", "plan-1", 1, "completed", "partial", "", Some(&result));
        assert!(text.contains("执行结果（编辑写入类）"));
        assert!(text.contains("验收：部分达成"));
        assert!(!text.contains("验收理由"));
    }

    #[test]
    fn result_text_none_result_guides_to_report() {
        let text = build_result_text("旧图", "plan-1", 1, "completed", "fail", "", None);
        assert!(text.contains("没有结构化执行结果"));
        assert!(text.contains("graph_plan_report"));
    }

    #[test]
    fn result_text_missing_conclusion_guides_to_nodes() {
        let result = result_of(RESULT_KIND_EDIT, Some("summary"), None, &[]);
        let text = build_result_text("失败图", "plan-1", 1, "failed", "fail", "汇总节点失败", Some(&result));
        assert!(text.contains("汇总节点未成功产出结论文本"));
        assert!(text.contains("graph_plan_report"));
    }

    #[test]
    fn result_text_truncates_oversized_conclusion() {
        let conclusion = "问".repeat(super::CONCLUSION_MAX_CHARS + 500);
        let result = result_of(RESULT_KIND_REVIEW, Some("summary"), Some(&conclusion), &[]);
        let text = build_result_text("大图", "plan-1", 1, "completed", "pass", "", Some(&result));
        assert!(text.contains("已截断"));
        // 截断后正文不超过上限（含提示行也远小于原文）。
        assert!(text.chars().count() < conclusion.chars().count());
        assert!(text.contains(&"问".repeat(super::CONCLUSION_MAX_CHARS)));
    }

    #[test]
    fn result_text_caps_modified_files_listing() {
        let files: Vec<String> = (0..45).map(|index| format!("src/f{index}.rs")).collect();
        let files_ref: Vec<&str> = files.iter().map(String::as_str).collect();
        let result = result_of(RESULT_KIND_EDIT, Some("n"), Some("结论"), &files_ref);
        let text = build_result_text("宽改图", "plan-1", 1, "completed", "pass", "", Some(&result));
        assert!(text.contains("修改文件（45 个）："));
        assert!(text.contains("…另有 5 个文件未列出"));
    }
}

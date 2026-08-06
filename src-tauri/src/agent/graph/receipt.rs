//! 执行回执（receipt）：run 收尾时把「验收结论 + 节点统计 + 共享 state + token 用量」
//! 组织成 markdown 消息写回会话，并广播会话更新——这是闭环编排的「回流」环节，
//! 让用户在聊天流直接看到结果，也让编排器下一轮能感知执行产出。

use std::collections::BTreeSet;

use serde_json::{Map, Value};
use tauri::{AppHandle, Emitter};

use super::types::{
    GraphNodeRunRecord, GraphPlanRecord, GraphRunSummary, NODE_FAILED, NODE_PHASE_CACHED,
    NODE_SKIPPED, NODE_SUCCEEDED, VERDICT_FAIL, VERDICT_PARTIAL, VERDICT_PASS,
};
use super::verifier::VerdictOutcome;
use crate::agent::db::{DispatcherDb, DispatcherMessageUsageStats};

/// 生成并投递执行回执。投递失败只记日志，不影响 run 终态。
pub(crate) async fn deliver_receipt(
    app: &AppHandle,
    db: &DispatcherDb,
    workspace_id: &str,
    plan: &GraphPlanRecord,
    run: &GraphRunSummary,
    node_runs: &[GraphNodeRunRecord],
    state: &Map<String, Value>,
    verdict: &VerdictOutcome,
) {
    // 本地 run 的 finished_at 在 finish_run 落库后仍未回填，用当前时刻兜底：
    // 回执紧随 finish_run 投递，二者相差可忽略。
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(run.started_at);
    let elapsed_ms = run
        .finished_at
        .unwrap_or(now_ms)
        .saturating_sub(run.started_at) as u64;
    let usage_stats = aggregate_usage(node_runs, verdict, elapsed_ms);
    let content = build_receipt_markdown(plan, run, node_runs, state, verdict, &usage_stats);
    let message = db
        .add_visible_message_with_usage_async(workspace_id, "assistant", &content, &usage_stats)
        .await;
    match message {
        Ok(_) => {
            // 回写会话 updated_at 并广播，聊天流/会话列表即时可见回执。
            if let Ok(Some(session)) = db.get_dispatcher_session_async(workspace_id).await {
                let _ = app.emit("dispatcher-session-updated", session);
            }
        }
        Err(error) => {
            eprintln!("[graph] 写入执行回执失败（{workspace_id}）：{error:#}");
        }
    }
}

fn verdict_badge(status: &str) -> &'static str {
    match status {
        VERDICT_PASS => "✅ 验收通过",
        VERDICT_PARTIAL => "🟡 部分达成",
        VERDICT_FAIL => "❌ 验收未通过",
        _ => "⚪ 未能验收",
    }
}

/// 外部内容（节点错误信息、共享 state 值、验收理由等）嵌入回执 markdown 前的
/// 中性化：换行/连续空白折叠为单个空格（多行堆栈会破坏回执所在的列表/段落
/// 结构），反引号替换为全角形式（避免意外撑开代码块、截断后续渲染）。
fn sanitize_for_markdown(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('`', "｀")
}

fn build_receipt_markdown(
    plan: &GraphPlanRecord,
    run: &GraphRunSummary,
    node_runs: &[GraphNodeRunRecord],
    state: &Map<String, Value>,
    verdict: &VerdictOutcome,
    usage_stats: &DispatcherMessageUsageStats,
) -> String {
    let mut succeeded = 0usize;
    let mut cached = 0usize;
    let mut failed = Vec::new();
    let mut skipped = Vec::new();
    let mut affected: BTreeSet<String> = BTreeSet::new();
    for run_record in node_runs {
        match run_record.status.as_str() {
            NODE_SUCCEEDED => {
                succeeded += 1;
                if run_record.phase == NODE_PHASE_CACHED {
                    cached += 1;
                }
                affected.extend(run_record.affected_files.iter().cloned());
            }
            NODE_FAILED => failed.push(sanitize_for_markdown(&run_record.node_id)),
            NODE_SKIPPED => skipped.push(sanitize_for_markdown(&run_record.node_id)),
            _ => {}
        }
    }

    let mut lines = vec![
        format!(
            "🗺️ 执行图《{}》第 {} 次运行结束 · {}",
            sanitize_for_markdown(&plan.title),
            run.attempt_no,
            verdict_badge(&verdict.status)
        ),
        String::new(),
    ];

    if !verdict.reason.trim().is_empty() {
        lines.push(format!(
            "**验收结论**：{}",
            sanitize_for_markdown(verdict.reason.trim())
        ));
        lines.push(String::new());
    }

    let cached_note = if cached > 0 {
        format!("（含断点续跑复用 {cached} 个）")
    } else {
        String::new()
    };
    lines.push(format!(
        "**节点统计**：成功 {succeeded}{cached_note} · 失败 {} · 跳过 {}",
        failed.len(),
        skipped.len()
    ));
    if !failed.is_empty() {
        lines.push(format!("  - 失败节点：{}", failed.join("、")));
        for run_record in node_runs
            .iter()
            .filter(|record| record.status == NODE_FAILED)
        {
            if let Some(error) = run_record.error_text.as_deref() {
                let error = sanitize_for_markdown(error);
                let error: String = error.chars().take(200).collect();
                lines.push(format!(
                    "  - `{}`：{error}",
                    sanitize_for_markdown(&run_record.node_id)
                ));
            }
        }
    }
    if !skipped.is_empty() {
        lines.push(format!("  - 跳过节点：{}", skipped.join("、")));
    }

    if !state.is_empty() {
        lines.push(String::new());
        lines.push("**共享 state**：".to_string());
        for (key, value) in state {
            let preview = value
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| value.to_string());
            let preview = sanitize_for_markdown(&preview);
            let preview: String = preview.chars().take(160).collect();
            lines.push(format!("- `{}`：{preview}", sanitize_for_markdown(key)));
        }
    }

    if !affected.is_empty() {
        lines.push(String::new());
        lines.push("**涉及文件**：".to_string());
        for file in affected.iter().take(40) {
            lines.push(format!("- `{}`", sanitize_for_markdown(file)));
        }
        if affected.len() > 40 {
            lines.push(format!("- …另有 {} 个文件", affected.len() - 40));
        }
    }

    lines.push(String::new());
    lines.push(format!(
        "**Token 用量**：输入 {} / 输出 {}（合计 {}）。",
        usage_stats.prompt_tokens, usage_stats.completion_tokens, usage_stats.total_tokens
    ));
    lines.join("\n")
}

/// 聚合本次 run 的 token 用量。cached（断点续跑复用）节点不计入本次消耗；
/// usage_json 形态容错（prompt/input、completion/output 等常见键名）。
/// `elapsed_ms`：run 的真实耗时（started_at→finished_at）。回执与主 agent 消息
/// 同属一个 user turn 时前端按字段取 max 聚合，若这里写 0，纯图编排轮次或
/// 回执先合并的场景会把 turn 级耗时钉在 0。
fn aggregate_usage(
    node_runs: &[GraphNodeRunRecord],
    verdict: &VerdictOutcome,
    elapsed_ms: u64,
) -> DispatcherMessageUsageStats {
    let mut prompt = 0u64;
    let mut completion = 0u64;
    for record in node_runs {
        if record.status != NODE_SUCCEEDED || record.phase == NODE_PHASE_CACHED {
            continue;
        }
        let (p, c) = parse_usage(&record.usage_json);
        prompt += p;
        completion += c;
    }
    if let Some(usage) = &verdict.usage {
        prompt += usage.prompt_tokens;
        completion += usage.completion_tokens;
    }
    DispatcherMessageUsageStats {
        prompt_tokens: prompt,
        completion_tokens: completion,
        total_tokens: prompt + completion,
        elapsed_ms,
        // 回执是运行收尾时一次性写入的消息，不属于主 agent 的暂停窗口，
        // per-message 语义上 false 即正确值；前端 turn 聚合也不合并 paused 字段。
        paused: false,
    }
}

/// 容错解析 usage JSON：兼容 OpenAI（prompt_tokens/completion_tokens）与
/// pi（input/output）等键名，返回 (prompt, completion)。
fn parse_usage(raw: &str) -> (u64, u64) {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return (0, 0);
    };
    let pick = |keys: &[&str]| -> u64 {
        for key in keys {
            if let Some(number) = value.get(key).and_then(Value::as_u64) {
                return number;
            }
        }
        0
    };
    let prompt = pick(&["prompt_tokens", "input_tokens", "input", "promptTokens"]);
    let completion = pick(&["completion_tokens", "output_tokens", "output", "completionTokens"]);
    (prompt, completion)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::graph::types::{BaseToolGroup, GraphNode};

    fn record(node_id: &str, status: &str, phase: &str, usage: &str) -> GraphNodeRunRecord {
        let node = GraphNode {
            id: node_id.into(),
            title: node_id.into(),
            role: String::new(),
            model_ref: "m1".into(),
            base_tool_group: BaseToolGroup::Coding,
            special_tools: vec![],
            task: "task".into(),
            depends_on: vec![],
            inject_state_keys: vec![],
            output_key: format!("out_{node_id}"),
            expected_files: vec![],
            export_policy: Default::default(),
        };
        let mut record = GraphNodeRunRecord::pending("run", "plan", &node);
        record.status = status.into();
        record.phase = phase.into();
        record.usage_json = usage.into();
        record
    }

    #[test]
    fn parse_usage_accepts_openai_and_pi_shapes() {
        assert_eq!(
            parse_usage(r#"{"prompt_tokens":10,"completion_tokens":5}"#),
            (10, 5)
        );
        assert_eq!(parse_usage(r#"{"input":3,"output":7}"#), (3, 7));
        assert_eq!(parse_usage("not-json"), (0, 0));
        assert_eq!(parse_usage("{}"), (0, 0));
    }

    #[test]
    fn aggregate_skips_cached_nodes() {
        let runs = vec![
            record("n1", "succeeded", "finalizing", r#"{"prompt_tokens":10,"completion_tokens":5}"#),
            record("n2", "succeeded", NODE_PHASE_CACHED, r#"{"prompt_tokens":100,"completion_tokens":50}"#),
            record("n3", "failed", "finalizing", "{}"),
        ];
        let verdict = VerdictOutcome {
            status: VERDICT_PASS.into(),
            reason: String::new(),
            usage: None,
        };
        let stats = aggregate_usage(&runs, &verdict, 0);
        assert_eq!(stats.prompt_tokens, 10, "cached 节点不计入");
        assert_eq!(stats.completion_tokens, 5);
        assert_eq!(stats.total_tokens, 15);
    }

    #[test]
    fn aggregate_backfills_elapsed_ms() {
        let runs = vec![record("n1", "succeeded", "finalizing", "{}")];
        let verdict = VerdictOutcome {
            status: VERDICT_PASS.into(),
            reason: String::new(),
            usage: None,
        };
        // elapsed_ms 由调用方按 run 的 started_at→finished_at 回填，
        // 不能恒为 0（会污染 turn 级 max 聚合的耗时展示）。
        let stats = aggregate_usage(&runs, &verdict, 42_000);
        assert_eq!(stats.elapsed_ms, 42_000);
        assert!(!stats.paused);
    }

    #[test]
    fn sanitize_neutralizes_newlines_and_backticks() {
        assert_eq!(
            sanitize_for_markdown("第一行\n第二行\t缩进  内容"),
            "第一行 第二行 缩进 内容"
        );
        assert_eq!(sanitize_for_markdown("带 `反引号` 的```围栏"), "带 ｀反引号｀ 的｀｀｀围栏");
        assert_eq!(sanitize_for_markdown("  前后空白  "), "前后空白");
    }

    #[test]
    fn receipt_error_and_state_do_not_break_markdown_structure() {
        let plan = GraphPlanRecord {
            id: "plan".into(),
            workspace_id: "w".into(),
            title: "测试图".into(),
            summary: String::new(),
            definition_json: "{}".into(),
            status: "failed".into(),
            state_json: "{}".into(),
            requirement: "需求".into(),
            inherits_plan_id: None,
            inherits_run_id: None,
            created_at: 0,
            updated_at: 0,
            latest_run_id: None,
            runs: vec![],
            node_runs: vec![],
        };
        let run = GraphRunSummary {
            id: "run".into(),
            plan_id: "plan".into(),
            attempt_no: 1,
            status: "failed".into(),
            mode: "full".into(),
            verdict_status: VERDICT_FAIL.into(),
            verdict_reason: String::new(),
            started_at: 0,
            finished_at: Some(1),
        };
        let mut failed_record = record("n1", "failed", "finalizing", "{}");
        failed_record.error_text = Some("编译失败\n```\nerror[E0308]\n```\n见 src/a.rs".into());
        let runs = vec![failed_record];
        let mut state = Map::new();
        state.insert(
            "analysis".into(),
            Value::String("多行\nstate `值`".into()),
        );
        let verdict = VerdictOutcome {
            status: VERDICT_FAIL.into(),
            reason: "节点失败\n结论".into(),
            usage: None,
        };
        let content = build_receipt_markdown(
            &plan,
            &run,
            &runs,
            &state,
            &verdict,
            &aggregate_usage(&runs, &verdict, 0),
        );
        // 错误/state/验收理由中的换行与反引号均被中性化，不再破坏列表结构。
        assert!(content.contains("  - `n1`：编译失败 ｀｀｀ error[E0308] ｀｀｀ 见 src/a.rs"));
        assert!(content.contains("- `analysis`：多行 state ｀值｀"));
        assert!(content.contains("**验收结论**：节点失败 结论"));
        assert!(!content.contains("编译失败\n"));
    }

    #[test]
    fn receipt_contains_verdict_and_stats() {
        let plan = GraphPlanRecord {
            id: "plan".into(),
            workspace_id: "w".into(),
            title: "测试图".into(),
            summary: String::new(),
            definition_json: "{}".into(),
            status: "completed".into(),
            state_json: "{}".into(),
            requirement: "需求".into(),
            inherits_plan_id: None,
            inherits_run_id: None,
            created_at: 0,
            updated_at: 0,
            latest_run_id: None,
            runs: vec![],
            node_runs: vec![],
        };
        let run = GraphRunSummary {
            id: "run".into(),
            plan_id: "plan".into(),
            attempt_no: 2,
            status: "completed".into(),
            mode: "full".into(),
            verdict_status: VERDICT_PASS.into(),
            verdict_reason: String::new(),
            started_at: 0,
            finished_at: Some(1),
        };
        let runs = vec![record("n1", "succeeded", "finalizing", "{}")];
        let verdict = VerdictOutcome {
            status: VERDICT_PASS.into(),
            reason: "产出满足需求".into(),
            usage: None,
        };
        let content = build_receipt_markdown(&plan, &run, &runs, &Map::new(), &verdict, &aggregate_usage(&runs, &verdict, 0));
        assert!(content.contains("测试图"));
        assert!(content.contains("验收通过"));
        assert!(content.contains("产出满足需求"));
        assert!(content.contains("成功 1"));
    }
}

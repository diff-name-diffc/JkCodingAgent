//! `prepare_rig_tool_result` 与 `RigToolResultPolicy` 的纯函数测试
//! （对齐 `common/tool_result.rs` 的行为契约）。

use super::*;

#[test]
fn empty_result_is_raw_mode() {
    let policy = RigToolResultPolicy::new(true);
    let prepared = prepare_rig_tool_result("read_file", &serde_json::json!({}), "   ", &policy);
    assert_eq!(prepared.result_mode, "raw");
    assert!(prepared.display_content.is_empty());
    assert!(prepared.context_payload.is_empty());
    assert!(!prepared.needs_summary);
    assert!(prepared.raw_output.is_empty());
}

#[test]
fn compress_requires_flag_and_threshold() {
    // 压缩声明 + 超阈值双条件才触发 pending_summary。
    let policy = RigToolResultPolicy::new(false);
    // 15000 字符：超 read_file 的 READ 档内联上限（10000）与压缩阈值（5000）。
    let long_output = "x".repeat(15_000);

    // 未声明 compress：即使超阈值也只截断。
    let prepared =
        prepare_rig_tool_result("read_file", &serde_json::json!({}), &long_output, &policy);
    assert_eq!(prepared.result_mode, "truncated");
    assert!(!prepared.needs_summary);

    // 声明 compress=true 且超阈值（默认 5000）：进入 pending_summary。
    let prepared = prepare_rig_tool_result(
        "read_file",
        &serde_json::json!({"compress": true}),
        &long_output,
        &policy,
    );
    assert_eq!(prepared.result_mode, "pending_summary");
    assert!(prepared.needs_summary);
    assert_eq!(prepared.raw_output.chars().count(), 15_000);
    // 摘要完成前不产出展示/上下文内容。
    assert!(prepared.display_content.is_empty());

    // 声明 compress=true 但未超阈值：直接 raw。
    let short_output = "x".repeat(100);
    let prepared = prepare_rig_tool_result(
        "read_file",
        &serde_json::json!({"compress": true}),
        &short_output,
        &policy,
    );
    assert_eq!(prepared.result_mode, "raw");
    assert!(!prepared.needs_summary);

    // 策略级 default_compress=true 时无需显式声明。
    let policy = RigToolResultPolicy::new(true);
    let prepared =
        prepare_rig_tool_result("read_file", &serde_json::json!({}), &long_output, &policy);
    assert_eq!(prepared.result_mode, "pending_summary");
}

#[test]
fn paged_read_tool_gets_larger_inline_budget() {
    let policy = RigToolResultPolicy::new(false);
    let output = "x".repeat(15_000);

    // read_file 无分页参数：READ 档（10000）→ 截断。
    let prepared = prepare_rig_tool_result("read_file", &serde_json::json!({}), &output, &policy);
    assert_eq!(prepared.result_mode, "truncated");

    // read_file 显式分页：PAGED 档（20000）→ 15000 字符原样内联。
    let prepared = prepare_rig_tool_result(
        "read_file",
        &serde_json::json!({"offset": 1, "limit": 200}),
        &output,
        &policy,
    );
    assert_eq!(prepared.result_mode, "raw");
    assert_eq!(prepared.context_payload.chars().count(), 15_000);

    // 非读取类工具：恒走默认 8000 → 截断。
    let prepared = prepare_rig_tool_result(
        "local_zsh",
        &serde_json::json!({"offset": 1}),
        &output,
        &policy,
    );
    assert_eq!(prepared.result_mode, "truncated");

    // 程序汇总结果使用更高的内联额度，截断说明不指向编排器读不到的工具产物。
    let program_output = "y".repeat(40_000);
    let prepared = prepare_rig_tool_result(
        "run_tool_program",
        &serde_json::json!({}),
        &program_output,
        &policy,
    );
    assert_eq!(prepared.result_mode, "truncated");
    assert!(prepared.context_payload.contains("编排器读不到工具产物"));
    assert!(!prepared.context_payload.contains("完整原始结果见工具产物"));
}

#[test]
fn exec_output_long_lines_are_line_shaped_before_truncation() {
    // 回归场景：ssh_exec 递归 grep 命中单行 base64 blob，砍头式整体截断会
    // 把后续证据行全部吞掉；行级截断削掉超长行后证据行得以全量保留。
    let policy = RigToolResultPolicy::new(false);
    let blob = "A".repeat(60_000);
    let output = format!(
        "=== header ===\n/data/a.xml:3:{blob}\n/data/b.yaml:12:embedding_model: bge-m3\n/data/c.yaml:9:embedding: gte-large"
    );
    let prepared = prepare_rig_tool_result("ssh_exec", &serde_json::json!({}), &output, &policy);
    assert_eq!(prepared.result_mode, "truncated");
    assert!(prepared.context_payload.contains("行级截断"));
    assert!(prepared.context_payload.contains("embedding_model: bge-m3"));
    assert!(prepared.context_payload.contains("embedding: gte-large"));
    // 超长行被削成头部 + 省略标记；整体落回内联预算，不再触发整体截断。
    assert!(!prepared.context_payload.contains(&"A".repeat(600)));
    assert!(!prepared.context_payload.contains("结果已截断"));
    // 完整原文仍进 raw_output（工具产物）。
    assert!(prepared.raw_output.chars().count() > 60_000);
}

#[test]
fn exec_output_still_truncated_when_line_shaping_is_not_enough() {
    let policy = RigToolResultPolicy::new(false);
    let blob = "B".repeat(60_000);
    let filler = "log line\n".repeat(2_000);
    let output = format!("{blob}\n{filler}");
    let prepared = prepare_rig_tool_result("ssh_exec", &serde_json::json!({}), &output, &policy);
    assert_eq!(prepared.result_mode, "truncated");
    // 行级截断与整体截断的标记同时出现，且提示语给出重取路径。
    assert!(prepared.context_payload.contains("行级截断"));
    assert!(prepared.context_payload.contains("结果已截断"));
    assert!(prepared
        .context_payload
        .contains("被截掉的部分不会回到上下文"));
}

#[test]
fn read_tool_output_is_not_line_shaped() {
    // 读取类工具的正文行需要逐字精读：不做行级截断，维持原有整体截断。
    let policy = RigToolResultPolicy::new(false);
    let long_line = "x".repeat(15_000);
    let prepared =
        prepare_rig_tool_result("read_file", &serde_json::json!({}), &long_line, &policy);
    assert_eq!(prepared.result_mode, "truncated");
    assert!(!prepared.context_payload.contains("行级截断"));
    assert!(prepared.context_payload.starts_with(&"x".repeat(10_000)));
}

#[test]
fn exec_output_without_long_lines_stays_untouched() {
    let policy = RigToolResultPolicy::new(false);
    let output = "short line\n".repeat(20);
    let prepared = prepare_rig_tool_result("ssh_exec", &serde_json::json!({}), &output, &policy);
    assert_eq!(prepared.result_mode, "raw");
    assert_eq!(prepared.context_payload, output.trim());
}

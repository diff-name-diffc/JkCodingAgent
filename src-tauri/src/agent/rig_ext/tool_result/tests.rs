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
    let prepared = prepare_rig_tool_result("read_file", &serde_json::json!({}), &long_output, &policy);
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
    let prepared = prepare_rig_tool_result("read_file", &serde_json::json!({}), &long_output, &policy);
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
    let prepared = prepare_rig_tool_result("local_zsh", &serde_json::json!({"offset": 1}), &output, &policy);
    assert_eq!(prepared.result_mode, "truncated");
}

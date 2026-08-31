//! 浏览器工具错误的分类与 LLM 感知的恢复策略。
//!
//! 引用失效自动抓取新快照让模型同轮重试；行为错误提示调整后重试；
//! 系统错误大声上报、不做自动恢复。

use serde_json::{json, Value};

use super::snapshot::{format_snapshot_response, READ_TEXT_DEFAULT_LINE_LIMIT};
use super::{run_browser_command_value, timeout_arg};
use crate::agent::tools::context::ToolContext;

/// Browser error classification for LLM-aware error handling.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum BrowserErrorKind {
    /// Element ref expired due to page navigation or DOM change.
    /// Recoverable: auto-fetch fresh snapshot and let LLM retry.
    RefExpired,
    /// System-level error (process crash, network failure, etc.).
    /// Fatal: report immediately, no auto-recovery.
    System,
    /// Transient behavioral error (timeout, page not ready, etc.).
    /// Recoverable: LLM can adjust strategy and retry.
    Behavioral,
}

pub(super) fn classify_browser_error(error: &str) -> BrowserErrorKind {
    if error.starts_with("[ref_expired]") {
        BrowserErrorKind::RefExpired
    } else if error.contains("超时")
        || error.contains("Timeout")
        || error.contains("timeout")
        || error.contains("尚未启动")
        || error.contains("not ready")
    {
        BrowserErrorKind::Behavioral
    } else {
        BrowserErrorKind::System
    }
}

/// When a ref-expired error is detected, automatically fetch a fresh accessibility
/// snapshot so the LLM receives up-to-date element refs in the same turn.
async fn auto_recover_snapshot(context: &ToolContext, original_error: &str) -> String {
    let recovery_result = run_browser_command_value(
        context,
        "read_text",
        json!({
            "ref": Value::Null,
            "maxNodes": 600,
            "timeout": timeout_arg(&json!({"timeout": 30_000}))
        }),
    )
    .await;

    match recovery_result {
        Ok(snapshot_value) => {
            // 复用 read_text 的行号分页渲染：恢复快照同样登记缓存，模型可直接用
            // offset/limit 接续读取，而不必整包重读。
            let snapshot_text = format_snapshot_response(
                snapshot_value,
                &context.workspace_id,
                None,
                1,
                READ_TEXT_DEFAULT_LINE_LIMIT,
            );
            format!(
                "错误：[ref_expired] {original_error}\n\n\
                ⚠️ 页面元素引用已失效（可能是页面发生了导航或内容变化）。\n\
                已自动获取最新页面快照，请基于以下新快照重新选择目标元素：\n\n\
                {snapshot_text}"
            )
        }
        Err(recovery_error) => {
            format!(
                "错误：[ref_expired] {original_error}\n\n\
                ⚠️ 页面元素引用已失效。尝试自动获取新快照时也失败了：{recovery_error}\n\
                请先调用 browser_read_text 获取最新快照，再重试操作。"
            )
        }
    }
}

/// Format a browser error with LLM-friendly guidance based on error classification.
pub(super) async fn handle_browser_error(context: &ToolContext, error: String) -> String {
    match classify_browser_error(&error) {
        BrowserErrorKind::RefExpired => auto_recover_snapshot(context, &error).await,
        BrowserErrorKind::Behavioral => {
            format!(
                "错误：{error}\n\n提示：这是一个可恢复的行为错误。请检查当前页面状态，\
                必要时重新调用 browser_read_text 获取最新快照后重试操作。"
            )
        }
        BrowserErrorKind::System => {
            // System errors: report loudly, no auto-recovery
            format!("错误：浏览器系统错误：{error}")
        }
    }
}
